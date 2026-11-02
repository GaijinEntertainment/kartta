mod uses;

use std::collections::{HashMap, HashSet};
use std::iter;
use std::ops::Range;

use self::uses::NameUse;
use kartta_lang::tree::{child_nodes, line, named_child_nodes, preorder, signature, text};
use kartta_lang::{
    CallRef, FILE_SCOPE, LocalCall, RawDirective, RawExtraction, Symbol, SymbolFlags, SymbolId,
    SymbolKind,
};
use tree_sitter::{Node, Tree};

use crate::banner::is_generated_source;
use crate::runtime::BUILTINS;

/// A from-import: its names are the imported names, then for each in turn the alias it is bound under, or an empty
/// string where it is bound under its own name.
pub(crate) const IMPORT_FORM: u32 = 0;
pub(crate) const FUTURE_FORM: u32 = 1;
/// A plain import of one module: its one name is the alias the module is bound under, and it has none where the import
/// binds the module's first segment.
pub(crate) const MODULE_FORM: u32 = 2;

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
        module_targets: HashSet::new(),
        module_uses: Vec::new(),
        builtin_calls: Vec::new(),
        type_names_end: 0,
        calls_end: 0,
        flags: SymbolFlags::empty().with_generated(is_generated_source(source)),
        raw: RawExtraction::default(),
    };

    preorder(tree.root_node(), |node| {
        if node.is_named() {
            emitter.handle(node);
        }
        true
    });
    emitter.close_frames(usize::MAX);
    emitter.record_module_uses();
    emitter.drop_builtin_calls();

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
    site_of: SymbolId,
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
    /// Where each name the module binds is written as bound, a site that is no use of it.
    module_targets: HashSet<usize>,
    module_uses: Vec<NameUse<'a>>,
    /// Where among the calls each bare call of a builtin's name that no scope around it binds sits, in ascending
    /// order: it is a call only where the module binds the name.
    builtin_calls: Vec<usize>,
    /// Where the last annotation or list of bases the walk entered ends: a name the walk meets before it names a type
    /// as one.
    type_names_end: usize,
    /// Where the farthest call the walk entered ends: a name the walk meets before it is an expression's, even inside
    /// an annotation or a list of bases.
    calls_end: usize,
    flags: SymbolFlags,
    raw: RawExtraction,
}

impl<'a> Emitter<'a> {
    fn handle(&mut self, node: Node<'_>) {
        self.close_frames(node.start_byte());
        let caller = self.frames.last().map_or(FILE_SCOPE, |frame| frame.caller);

        match node.kind() {
            FUNCTION_DEFINITION => {
                if let Some(id) = self.function(node) {
                    self.define(node, id);
                    self.open(node, id, FrameKind::Function);
                    self.open_receiver(node, id);
                }
            }
            CLASS_DEFINITION => {
                let id = self.class(node);
                if let Some(id) = id {
                    self.define(node, id);
                }
                if let Some(bases) = node.child_by_field_name("superclasses") {
                    self.type_names_end = self.type_names_end.max(bases.end_byte());
                }
                self.open(node, id.unwrap_or(caller), FrameKind::Class);
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
            "type" => self.type_names_end = self.type_names_end.max(node.end_byte()),
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

    // constraint: a comprehension evaluates its first iterable in the scope around it, and a class its bases
    fn open(&mut self, node: Node<'_>, caller: SymbolId, kind: FrameKind) {
        let outside = match kind {
            FrameKind::Comprehension => named_child_nodes(node)
                .find(|child| child.kind() == "for_in_clause")
                .and_then(|clause| clause.child_by_field_name("right")),
            FrameKind::Class => node.child_by_field_name("superclasses"),
            FrameKind::Function => None,
        }
        .map_or(0..0, |evaluated| evaluated.byte_range());
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
        self.settle_uses(
            &bindings,
            (kind == FrameKind::Class).then_some(caller),
            uses,
        );
        let binds_locals = kind == FrameKind::Function || self.in_function();
        for call in calls {
            let binding = bindings
                .iter()
                .filter(|&&(name, _)| name == call.callee)
                .map(|&(_, binding)| binding)
                .max_by_key(|binding| binding.strength());
            match binding {
                Some(Binding::Definition(local)) if binds_locals => {
                    self.raw
                        .local_calls
                        .push(LocalCall::new(call.site_of, local, call.line));
                }
                Some(Binding::Definition(_) | Binding::Imported) => self.push_call(&call),
                Some(Binding::Module) => self.push_free_call(&call),
                Some(Binding::Value) => {}
                Some(Binding::Enclosing) | None => match self.scope_frame(call.at) {
                    Some(outer) => outer.calls.push(call),
                    None => self.push_free_call(&call),
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
        let Some(name) = node.child_by_field_name("name") else {
            return;
        };
        let frame = self
            .frames
            .iter_mut()
            .rev()
            .find(|frame| frame.kind != FrameKind::Comprehension);
        match frame {
            Some(frame) => frame
                .bindings
                .push((&self.source[name.byte_range()], Binding::Value)),
            None => self.bind_in_module(name, Binding::Value),
        }
    }

    // constraint: only a value's target is read as a name; a definition's name and an import's are not
    fn bind_in_module(&mut self, name: Node<'_>, binding: Binding) {
        self.module_names
            .insert(&self.source[name.byte_range()], binding);
        if matches!(binding, Binding::Value) {
            self.module_targets.insert(name.start_byte());
        }
    }

    // constraint: the grammar spells the alias of a `with` or `except` target as a field, and that of a `case` pattern
    // as its last child
    fn bind_alias(&mut self, node: Node<'_>) {
        let alias = node.child_by_field_name("alias").or_else(|| {
            named_child_nodes(node)
                .next_back()
                .filter(|last| last.kind() == IDENTIFIER)
        });
        if let Some(alias) = alias {
            self.bind(alias, Binding::Value);
        }
    }

    // constraint: a `case` pattern captures a bare name and compares against a dotted one, and a keyword pattern's own
    // keyword is no capture
    fn bind_capture(&mut self, node: Node<'_>) {
        let captured = named_child_nodes(node)
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
                if let Some(frame) = self.frames.last_mut() {
                    frame
                        .bindings
                        .push((&self.source[target.byte_range()], binding));
                } else {
                    self.bind_in_module(target, binding);
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
                for child in named_child_nodes(target) {
                    self.bind(child, binding);
                }
            }
            _ => {}
        }
    }

    // constraint: a definition inside a function body, a class and its members included, is local to that body
    fn push_symbol(&mut self, mut symbol: Symbol) -> SymbolId {
        symbol.flags = symbol.flags.union(self.flags);
        let id = self.raw.push_symbol(symbol);
        if self.in_function() {
            self.raw.local_symbols.push(id);
        }
        id
    }

    fn in_function(&self) -> bool {
        self.frames
            .iter()
            .any(|frame| frame.kind == FrameKind::Function)
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
        if let Some(class) = classes_around(node).next() {
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
        if let Some(class) = classes_around(node).next() {
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
        // constraint: `codegen_annassign` stores no value for an annotation without one, so the class's `__dict__`
        // holds no such name and a parent's member stays what a lookup finds
        if node.child_by_field_name("right").is_none() {
            symbol.value_types = vec![String::new()];
        }
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
        let shadows = named_child_nodes(parameters)
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

    fn call(&mut self, node: Node<'_>, site_of: SymbolId) {
        self.calls_end = self.calls_end.max(node.end_byte());
        let line = line(node.start_position().row);
        let at = node.start_byte();
        let source = self.source;
        if let Some(function) = node
            .child_by_field_name("function")
            .filter(|function| function.kind() == IDENTIFIER)
            && let Some(frame) = self.frames.iter_mut().rev().find(|frame| frame.holds(at))
        {
            frame.calls.push(BareCall {
                site_of,
                callee: &source[function.byte_range()],
                line,
                at,
            });
            return;
        }
        let Some((callee, receiver)) = callee(node, self.source) else {
            return;
        };
        if receiver.is_none() {
            self.mark_builtin(callee.as_bytes());
        }

        self.raw
            .calls
            .push(CallRef::new(site_of, callee, receiver, line));
    }

    fn mark_builtin(&mut self, callee: &[u8]) {
        if BUILTINS
            .binary_search_by(|builtin| builtin.as_bytes().cmp(callee))
            .is_ok()
        {
            self.builtin_calls.push(self.raw.calls.len());
        }
    }

    // constraint: a bare call of a name that neither its scopes nor its module bind reaches the builtin of that name,
    // and the graph would bind it to a function of the name in another file
    fn drop_builtin_calls(&mut self) {
        let mut builtin_calls = self.builtin_calls.iter().copied().peekable();
        let mut at = 0;
        self.raw.calls.retain(|call| {
            let builtin = builtin_calls.next_if_eq(&at).is_some();
            at += 1;
            !builtin || self.module_names.contains_key(call.callee.as_bytes())
        });
    }

    fn push_free_call(&mut self, call: &BareCall<'_>) {
        self.mark_builtin(call.callee);
        self.push_call(call);
    }

    fn push_call(&mut self, call: &BareCall<'_>) {
        self.raw.calls.push(CallRef::new(
            call.site_of,
            String::from_utf8_lossy(call.callee).into_owned(),
            None,
            call.line,
        ));
    }

    fn import(&mut self, node: Node<'_>) {
        for child in named_child_nodes(node) {
            let Some(module) = imported_name(child, self.source) else {
                continue;
            };
            let mut directive =
                RawDirective::new(MODULE_FORM, module, line(node.start_position().row));
            directive.names.extend(alias(child, self.source));
            self.raw.directives.push(directive);
        }
    }

    // constraint: a wildcard import leaves the names empty, which the resolver reads as the module alone, as it reads
    // a from-import that names nothing
    fn import_from(&mut self, node: Node<'_>) {
        let Some(module) = node.child_by_field_name("module_name") else {
            return;
        };
        let mut cursor = node.walk();
        let mut names: Vec<String> = node
            .children_by_field_name("name", &mut cursor)
            .filter_map(|name| imported_name(name, self.source))
            .collect();
        let aliases = node
            .children_by_field_name("name", &mut cursor)
            .filter(|name| matches!(name.kind(), "dotted_name" | "aliased_import"))
            .map(|name| alias(name, self.source).unwrap_or_default());
        names.extend(aliases);

        let mut directive = RawDirective::new(
            IMPORT_FORM,
            text(module, self.source),
            line(node.start_position().row),
        );
        directive.names = names;
        self.raw.directives.push(directive);
    }

    // constraint: a future feature cannot be aliased, so its names are read as written
    fn future_import(&mut self, node: Node<'_>) {
        let mut cursor = node.walk();
        let names = node
            .children_by_field_name("name", &mut cursor)
            .map(|name| text(name, self.source))
            .collect();

        let mut directive = RawDirective::new(
            FUTURE_FORM,
            "__future__".to_owned(),
            line(node.start_position().row),
        );
        directive.names = names;
        self.raw.directives.push(directive);
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

fn alias(node: Node<'_>, source: &[u8]) -> Option<String> {
    node.child_by_field_name("alias")
        .map(|alias| text(alias, source))
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
    decorators(function).any(|written| {
        let written = text(written, source);
        let (object, last) = written.rsplit_once('.').unwrap_or(("", &written));
        matches!(last, "property" | "cached_property")
            || (matches!(last, "getter" | "setter" | "deleter") && object == name)
    })
}

fn is_static(function: Node<'_>, source: &[u8]) -> bool {
    decorators(function)
        .any(|written| text(written, source).rsplit('.').next() == Some("staticmethod"))
}

fn decorators(function: Node<'_>) -> impl Iterator<Item = Node<'_>> {
    named_child_nodes(decorated(function))
        .filter(|decorator| decorator.kind() == "decorator")
        .filter_map(|decorator| decorator.named_child(0))
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

// constraint: the chain stops at the nearest function, so a definition nested in a function is never a member of a
// class around that function
fn classes_around(node: Node<'_>) -> impl Iterator<Item = Node<'_>> {
    iter::successors(node.parent(), Node::parent)
        .take_while(|ancestor| ancestor.kind() != FUNCTION_DEFINITION)
        .filter(|ancestor| ancestor.kind() == CLASS_DEFINITION)
}

fn enclosing_classes(node: Node<'_>, source: &[u8]) -> Option<String> {
    let mut names = classes_around(node)
        .map(|class| Some(text(class.child_by_field_name("name")?, source)))
        .collect::<Option<Vec<_>>>()?;
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

    child_nodes(arguments)
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
