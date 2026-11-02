use std::collections::HashMap;

use kartta_lang::tree::text;
use kartta_lang::{Access, CallRef, Receiver, UseRef};
use tree_sitter::Node;

use super::data::data_declarator;
use super::declarator::qualified_terminal;
use super::{Replayer, line};

/// The names the bodies around the walk declare, each visible from its declaration to the end of the block that
/// declares it, the latest of a name shadowing the ones before it. A name is declared by every declarator the crate's
/// data symbols follow, a reference declarator's unnamed child, a range-for, a structured binding and an init-capture.
#[derive(Default)]
pub(super) struct Locals<'s> {
    declared: Vec<Local<'s>>,
    latest: HashMap<&'s str, usize>,
}

struct Local<'s> {
    name: &'s str,
    end: usize,
    holds: Holds<'s>,
    shadows: Option<usize>,
}

/// What a declared name holds, as far as its declaration spells it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Holds<'s> {
    /// A value of the owner its type names, or a reference to one.
    Value(&'s str),
    /// A pointer to a value of the owner its type names.
    Pointer(&'s str),
    /// What no owner names: `auto`, a builtin, a template parameter.
    Unknown,
}

impl<'s> Locals<'s> {
    fn declare(&mut self, name: &'s str, end: usize, holds: Holds<'s>) {
        let shadows = self.latest.insert(name, self.declared.len());
        self.declared.push(Local {
            name,
            end,
            holds,
            shadows,
        });
    }

    pub(super) fn close(&mut self, start: usize) {
        while let Some(local) = self.declared.pop_if(|local| local.end <= start) {
            match local.shadows {
                Some(index) => self.latest.insert(local.name, index),
                None => self.latest.remove(local.name),
            };
        }
    }

    pub(super) fn declares(&self, name: &str) -> bool {
        self.latest.contains_key(name)
    }

    fn holds(&self, name: &str) -> Option<Holds<'s>> {
        self.latest
            .get(name)
            .map(|&index| self.declared[index].holds)
    }
}

const BLOCKS: [&str; 11] = [
    "compound_statement",
    "for_statement",
    "for_range_loop",
    "if_statement",
    "while_statement",
    "do_statement",
    "switch_statement",
    "case_statement",
    "catch_clause",
    "lambda_expression",
    "function_definition",
];

impl<'s, 't> Replayer<'s, 't> {
    pub(super) fn handle_in_body(&mut self, node: Node<'t>) {
        match node.kind() {
            "declaration" => {
                if let Some(block) = self
                    .ancestors
                    .iter()
                    .rev()
                    .find(|ancestor| BLOCKS.contains(&ancestor.kind()))
                {
                    self.declare(node, block.end_byte());
                }
            }
            "parameter_declaration"
            | "optional_parameter_declaration"
            | "variadic_parameter_declaration" => {
                if let Some(end) = self.parameter_scope_end() {
                    self.declare(node, end);
                }
            }
            "for_range_loop" => self.declare(node, node.end_byte()),
            "lambda_capture_initializer" => self.declare_capture(node),
            _ if !self.in_body(node) => {}
            "identifier" => self.record_name(node),
            "qualified_identifier" => self.record_qualified(node),
            "field_expression" => self.record_member(node),
            "field_initializer" => self.record_initialized_field(node),
            "initializer_pair" => self.record_designated_field(node),
            _ => {}
        }
    }

    // constraint: an initializer outside every body names a value or a function the file's scope reaches, as a table
    // of handlers does; the use has no symbol to resolve from, so the graph keeps only the name, and a lambda there
    // declares locals the walk does not follow
    pub(super) fn handle_at_file_scope(&mut self, node: Node<'t>) {
        if !matches!(node.kind(), "identifier" | "qualified_identifier") {
            return;
        }
        let Some(&parent) = self.ancestors.last() else {
            return;
        };
        let used = if parent.kind() == "field_declaration" {
            fills(parent, "default_value", node)
        } else {
            is_used(node, parent)
        };
        if !used || !self.in_file_scope_initializer(node) {
            return;
        }
        let name = if node.kind() == "identifier" {
            node
        } else {
            match qualified_terminal(node).0 {
                Some(terminal) if terminal.kind() == "identifier" => terminal,
                _ => return,
            }
        };
        let access = self.access_of(node);
        self.push_site(name, (text(name, self.source), Receiver::Bare, access));
    }

    fn in_file_scope_initializer(&self, node: Node<'_>) -> bool {
        let mut child = node;
        for &ancestor in self.ancestors.iter().rev() {
            match ancestor.kind() {
                "lambda_expression" => return false,
                "init_declarator" => return fills(ancestor, "value", child),
                "field_declaration" => return fills(ancestor, "default_value", child),
                _ => child = ancestor,
            }
        }
        false
    }

    // constraint: a definition's head before its initializer list and body reads no value; what the grammar finds
    // there is most often an annotation macro read as a name, `PX_FORCE_INLINE void f()`
    fn in_body(&self, node: Node<'_>) -> bool {
        self.functions
            .last()
            .is_some_and(|function| node.start_byte() >= function.body)
    }

    // constraint: a parameter list reaches past its declarator only for a function's body, a lambda's or a handler's;
    // the parameters of a prototype, of a function pointer or of a template declare nothing a body reads
    fn parameter_scope_end(&self) -> Option<usize> {
        self.ancestors
            .iter()
            .rev()
            .find(|ancestor| {
                ancestor.kind() != "parameter_list" && !ancestor.kind().ends_with("declarator")
            })
            .filter(|scope| {
                matches!(
                    scope.kind(),
                    "function_definition" | "lambda_expression" | "catch_clause"
                )
            })
            .map(Node::end_byte)
    }

    fn declare(&mut self, declaration: Node<'_>, end: usize) {
        let owner = declaration
            .child_by_field_name("type")
            .and_then(|written| self.owner_spelled(written));
        let mut cursor = declaration.walk();
        for declarator in declaration.children_by_field_name("declarator", &mut cursor) {
            self.declare_names(declarator, owner, end);
        }
    }

    fn declare_names(&mut self, declarator: Node<'_>, owner: Option<&'s str>, end: usize) {
        let (mut declarator, mut owner) = (declarator, owner);
        loop {
            if let Some((name, pointer)) = data_declarator(declarator, self.source) {
                let holds = match (owner, pointer) {
                    (Some(owner), None) => Holds::Value(owner),
                    (Some(owner), Some(_)) => Holds::Pointer(owner),
                    (None, _) => Holds::Unknown,
                };
                return self.declare_name(name, end, holds);
            }
            let inner = match declarator.kind() {
                "structured_binding_declarator" => {
                    return self.declare_bound_names(declarator, end);
                }
                "init_declarator" => declarator.child_by_field_name("declarator"),
                "reference_declarator"
                | "variadic_declarator"
                | "attributed_declarator"
                | "parenthesized_declarator" => {
                    owner = None;
                    declarator.named_child(0)
                }
                _ => None,
            };
            let Some(inner) = inner else {
                return;
            };
            declarator = inner;
        }
    }

    fn declare_name(&mut self, name: Node<'_>, end: usize, holds: Holds<'s>) {
        if let Some(name) = self.borrowed(name) {
            self.locals.declare(name, end, holds);
        }
    }

    fn declare_bound_names(&mut self, declarator: Node<'_>, end: usize) {
        let source = self.source;
        let mut cursor = declarator.walk();
        let names = declarator
            .named_children(&mut cursor)
            .filter_map(|name| borrowed(source, name));
        for name in names {
            self.locals.declare(name, end, Holds::Unknown);
        }
    }

    fn declare_capture(&mut self, capture: Node<'_>) {
        let lambda = self
            .ancestors
            .iter()
            .rev()
            .find(|ancestor| ancestor.kind() == "lambda_expression");
        if let (Some(lambda), Some(name)) = (lambda, capture.child_by_field_name("left"))
            && let Some(name) = self.borrowed(name)
        {
            self.locals.declare(name, lambda.end_byte(), Holds::Unknown);
        }
    }

    // constraint: `->` on a value of a class template calls its `operator->`, which a smart pointer, an optional or a
    // handle aims at the class its first argument names, `Lock` of `eastl::unique_ptr<Lock>`, and `[]` calls its
    // `operator[]`, which a sequence aims at the same class, `Lock` of `Tab<Lock>`; a template without the operator
    // does not compile under it, so no call reads the class through one
    /// The class a class template's first argument names, and how many pointers from a value of the template it
    /// lies: one, and one more for an argument written as a pointer, `Tab<Lock *>`.
    pub(super) fn pointee_named(&self, written: Node<'_>) -> Option<(&'s str, usize)> {
        let mut template = written;
        while template.kind() == "qualified_identifier" {
            template = template.child_by_field_name("name")?;
        }
        let first = template
            .child_by_field_name("arguments")
            .filter(|_| template.kind() == "template_type")?
            .named_child(0)
            .filter(|first| first.kind() == "type_descriptor")?;
        let pointers = match first.child_by_field_name("declarator") {
            None => 1,
            Some(declarator)
                if declarator.kind() == "abstract_pointer_declarator"
                    && declarator.child_by_field_name("declarator").is_none() =>
            {
                2
            }
            Some(_) => return None,
        };
        self.owner_named(first.child_by_field_name("type")?)
            .map(|class| (class, pointers))
    }

    /// The class a type names as a field's value type spells it: with the scopes it writes and without its template's
    /// arguments, which a list of value types joined at a comma cannot hold.
    pub(super) fn owner_named(&self, written: Node<'_>) -> Option<&'s str> {
        let (spelled, _, name) = owner_type(written)?;
        self.borrowed(name)
            .filter(|owner| !self.is_template_parameter(owner))
            .map(|owner| self.scoped(spelled, name).unwrap_or(owner))
    }

    /// The class a type names as a receiver spells it: with the scopes and the template's arguments it writes,
    /// `ns::Box<bool>`, which tell a specialization apart from its primary template.
    fn owner_spelled(&self, written: Node<'_>) -> Option<&'s str> {
        let (spelled, end, name) = owner_type(written)?;
        self.borrowed(name)
            .filter(|owner| !self.is_template_parameter(owner))
            .map(|owner| self.argued(spelled, end).unwrap_or(owner))
    }

    // constraint: a class keeps the scopes its spelling writes, `b::Widget` or `::b::Widget`, which tell namesakes of
    // two namespaces apart; a spelling that holds anything but names and `::`, a template's arguments or a space, is
    // left to the bare name
    fn scoped(&self, from: Node<'_>, name: Node<'_>) -> Option<&'s str> {
        std::str::from_utf8(&self.source[from.start_byte()..name.end_byte()])
            .ok()
            .filter(|spelled| spelled.bytes().all(is_name_byte))
    }

    // constraint: a receiver keeps the arguments its scopes' templates take, whatever they hold, as well as the scopes;
    // a spelling that holds anything else outside them but names and `::`, a space most often, is left to the bare
    // name
    fn argued(&self, from: Node<'_>, to: Node<'_>) -> Option<&'s str> {
        std::str::from_utf8(&self.source[from.start_byte()..to.end_byte()])
            .ok()
            .filter(|spelled| is_argued_name(spelled))
    }

    // constraint: a name the file's walk cannot see declared is kept, since an included file may declare it, and the
    // graph binds only the ones some file declares
    fn record_name(&mut self, node: Node<'t>) {
        let Some(name) = self.borrowed(node) else {
            return;
        };
        if self.locals.holds(name).is_some() || self.is_template_parameter(name) {
            return;
        }
        if self
            .ancestors
            .last()
            .is_some_and(|&parent| is_used(node, parent))
        {
            let access = self.access_of(node);
            self.push_site(node, (name.to_owned(), Receiver::Bare, access));
        }
    }

    // constraint: a qualified name names a member of the class or the namespace its scope spells, so the use is on the
    // scope as a declaration's class is spelled, up to the scope's last segment; a name written from the root alone is
    // bare
    fn record_qualified(&mut self, node: Node<'t>) {
        let Some(&parent) = self.ancestors.last() else {
            return;
        };
        if !is_used(node, parent) {
            return;
        }
        let (Some(name), scope) = qualified_terminal(node) else {
            return;
        };
        if name.kind() != "identifier" {
            return;
        }
        let receiver = match scope {
            None => Receiver::Bare,
            Some(scope) => {
                let segment = match scope.kind() {
                    "namespace_identifier" | "type_identifier" => Some(scope),
                    "template_type" => scope.child_by_field_name("name"),
                    _ => None,
                };
                let Some(segment) = segment else {
                    return;
                };
                Receiver::Path(
                    self.argued(node, scope)
                        .map_or_else(|| text(segment, self.source), str::to_owned),
                )
            }
        };
        let access = self.access_of(node);
        self.push_site(name, (text(name, self.source), receiver, access));
    }

    // constraint: the operand of `.` or `->` names a type only where the walk saw it declared as a value of one, or
    // a pointer to one for `->`; any other operand records the empty path
    fn record_member(&mut self, node: Node<'t>) {
        let (Some(operand), Some(field)) = (
            node.child_by_field_name("argument"),
            node.child_by_field_name("field")
                .filter(|field| field.kind() == "field_identifier"),
        ) else {
            return;
        };
        let arrow = match node
            .child_by_field_name("operator")
            .map(|operator| operator.kind())
        {
            Some("->") => true,
            Some(".") => false,
            _ => return,
        };
        let called = self.ancestors.last().is_some_and(|&parent| {
            parent.kind() == "call_expression" && fills(parent, "function", node)
        });
        if called {
            return;
        }
        let receiver = match operand.kind() {
            "this" => Receiver::Own,
            "identifier" => Receiver::Path(self.declared_owner(operand, arrow).to_owned()),
            _ => Receiver::Path(String::new()),
        };
        let access = self.access_of(node);
        self.push_site(field, (text(field, self.source), receiver, access));
    }

    /// The receiver of a member call on a named operand: the class a local or a parameter is declared as, as
    /// `declared_owner` reads it, and in a member the name of what the body does not declare, `lock` or `this->lock`,
    /// after the operator, `.lock` or `->lock`, which may be a field of the caller's class or of a base, or a
    /// namespace's value, for the graph to tell apart, before a `[]` for an element of it, `locks[i]` as `.locks[]`;
    /// else empty.
    pub(super) fn called_owner(&self, call: Node<'_>) -> String {
        let Some(function) = call
            .child_by_field_name("function")
            .filter(|function| function.kind() == "field_expression")
        else {
            return String::new();
        };
        let (Some(operand), Some(operator)) = (
            function.child_by_field_name("argument"),
            function.child_by_field_name("operator"),
        ) else {
            return String::new();
        };
        let arrow = operator.kind() == "->";
        let in_member = self.callers_owner().is_some();
        let (operand, element) = match operand.kind() {
            "subscript_expression" => match operand.child_by_field_name("argument") {
                Some(indexed) => (indexed, "[]"),
                None => return String::new(),
            },
            _ => (operand, ""),
        };
        let field = match operand.kind() {
            "identifier" => match self.borrowed(operand) {
                Some(name)
                    if in_member
                        && !self.locals.declares(name)
                        && !self.is_template_parameter(name) =>
                {
                    name
                }
                _ if element.is_empty() => return self.declared_owner(operand, arrow).to_owned(),
                _ => return String::new(),
            },
            "field_expression" if in_member => match self.field_of_this(operand) {
                Some(name) => name,
                None => return String::new(),
            },
            _ => return String::new(),
        };
        let operator = if arrow { "->" } else { "." };
        format!("{operator}{field}{element}")
    }

    /// The field `this->field` names.
    fn field_of_this(&self, operand: Node<'_>) -> Option<&'s str> {
        let on_this = operand
            .child_by_field_name("argument")
            .is_some_and(|argument| argument.kind() == "this")
            && operand
                .child_by_field_name("operator")
                .is_some_and(|operator| operator.kind() == "->");
        operand
            .child_by_field_name("field")
            .filter(|field| on_this && field.kind() == "field_identifier")
            .and_then(|field| self.borrowed(field))
    }

    fn declared_owner(&self, operand: Node<'_>, arrow: bool) -> &'s str {
        let held = self
            .borrowed(operand)
            .and_then(|name| self.locals.holds(name));
        match (held, arrow) {
            (Some(Holds::Value(owner)), false) | (Some(Holds::Pointer(owner)), true) => owner,
            _ => "",
        }
    }

    fn record_initialized_field(&mut self, node: Node<'_>) {
        let Some(written) = node.named_child(0) else {
            return;
        };
        if written.kind() == "field_identifier" {
            let name = text(written, self.source);
            if self.class_derives_from(&name) {
                self.push_base_call(name.clone(), written);
            }
            self.push_site(written, (name, Receiver::Own, Access::Write));
        } else if let Some(base) = scoped_initializer(written) {
            let name = text(base, self.source);
            let names_a_namesake = written.kind() == "qualified_identifier"
                && self.callers_owner() == Some(name.as_str());
            let callee = if names_a_namesake {
                let class = &self.source[written.start_byte()..base.end_byte()];
                format!("{}::{name}", String::from_utf8_lossy(class))
            } else {
                name
            };
            self.push_base_call(callee, written);
        }
    }

    // constraint: a constructor defined outside its class body sees no class, which usually lives in a header, so its
    // bare names stay writes
    fn class_derives_from(&self, name: &str) -> bool {
        self.classes.last().is_some_and(|scope| {
            self.raw.symbols[scope.id.0 as usize]
                .parents
                .iter()
                .any(|parent| parent.rsplit("::").next() == Some(name))
        })
    }

    // constraint: an initializer of the caller's own name delegates, or names another specialization of the caller's
    // template as its base, and a bare call of that name binds the caller
    fn push_base_call(&mut self, callee: String, at: Node<'_>) {
        if self.is_template_parameter(&callee) || self.callers_owner() == Some(callee.as_str()) {
            return;
        }
        self.raw.calls.push(CallRef::new(
            self.caller(),
            callee,
            None,
            line(at.start_position().row),
        ));
    }

    fn record_designated_field(&mut self, node: Node<'_>) {
        let field = node
            .child_by_field_name("designator")
            .and_then(|designator| match designator.kind() {
                "field_identifier" => Some(designator),
                "field_designator" => designator.named_child(0),
                _ => None,
            });
        if let Some(field) = field {
            let owner = self.aggregate_owner().unwrap_or_default().to_owned();
            let name = text(field, self.source);
            self.push_site(field, (name, Receiver::Path(owner), Access::Write));
        }
    }

    // constraint: only a list that initializes a declared name of a type, or a compound literal, spells the type its
    // designators name fields of; a nested list or a returned one does not
    fn aggregate_owner(&self) -> Option<&'s str> {
        let mut up = self.ancestors.iter().rev();
        let list = up.next().filter(|list| list.kind() == "initializer_list")?;
        let holder = up.next()?;
        let written = match holder.kind() {
            "compound_literal_expression" => holder.child_by_field_name("type"),
            "init_declarator"
                if fills(*holder, "value", *list)
                    && holder
                        .child_by_field_name("declarator")
                        .is_some_and(|name| name.kind() == "identifier") =>
            {
                up.next()
                    .filter(|declaration| declaration.kind() == "declaration")
                    .and_then(|declaration| declaration.child_by_field_name("type"))
            }
            _ => None,
        };
        self.owner_spelled(written?)
    }

    // constraint: only the outermost expression on the left of an assignment is assigned; `a.b` in `a.b.c = v` is read
    fn access_of(&self, node: Node<'_>) -> Access {
        let mut at = node;
        for &holder in self.ancestors.iter().rev() {
            match holder.kind() {
                "parenthesized_expression" => at = holder,
                "update_expression" => return Access::Update,
                "assignment_expression" if fills(holder, "left", at) => {
                    return assignment_access(holder);
                }
                _ => return Access::Read,
            }
        }
        Access::Read
    }

    fn push_site(&mut self, at: Node<'_>, (name, receiver, access): (String, Receiver, Access)) {
        let mut site = UseRef::new(self.caller(), name, receiver, line(at.start_position().row));
        site.access = access;
        self.raw.uses.push(site);
    }

    fn borrowed(&self, node: Node<'_>) -> Option<&'s str> {
        borrowed(self.source, node)
    }
}

/// Where the part of a function definition that reads values starts: its initializer list, else its body, else its
/// end.
pub(super) fn body_start(definition: Node<'_>) -> usize {
    let mut cursor = definition.walk();
    let initializers = definition
        .named_children(&mut cursor)
        .find(|child| child.kind() == "field_initializer_list");
    initializers
        .or_else(|| definition.child_by_field_name("body"))
        .map_or_else(|| definition.end_byte(), |body| body.start_byte())
}

/// The bare name of a base an initializer spells with a scope or a template's arguments, `B` for `ns::B<int>`, which a
/// field's name takes neither of.
fn scoped_initializer(written: Node<'_>) -> Option<Node<'_>> {
    let mut written = written;
    while matches!(written.kind(), "qualified_identifier" | "template_method") {
        written = written.child_by_field_name("name")?;
    }
    (written.kind() == "field_identifier").then_some(written)
}

fn borrowed<'s>(source: &'s [u8], node: Node<'_>) -> Option<&'s str> {
    std::str::from_utf8(&source[node.byte_range()]).ok()
}

fn assignment_access(assignment: Node<'_>) -> Access {
    let plain = assignment
        .child_by_field_name("operator")
        .is_none_or(|operator| operator.kind() == "=");
    if plain { Access::Write } else { Access::Update }
}

const fn is_name_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_' || byte == b':'
}

/// Whether a spelling holds only names and `::` outside its closed template argument lists, where a `>` inside
/// parentheses, `Less<(1 > 2)>`, closes none.
fn is_argued_name(spelled: &str) -> bool {
    let (mut angles, mut parentheses) = (0_usize, 0_usize);
    let outside = spelled.bytes().all(|byte| match byte {
        b'(' if angles > 0 => {
            parentheses += 1;
            true
        }
        b')' if parentheses > 0 => {
            parentheses -= 1;
            true
        }
        b'<' if parentheses == 0 => {
            angles += 1;
            true
        }
        b'>' if angles > 0 && parentheses == 0 => {
            angles -= 1;
            true
        }
        _ => angles > 0 || is_name_byte(byte),
    });
    outside && angles == 0
}

/// The nodes a class's spelling starts and ends at, its scopes and its template's arguments included, and the class's
/// bare name.
fn owner_type(written: Node<'_>) -> Option<(Node<'_>, Node<'_>, Node<'_>)> {
    let mut written = written;
    let (mut start, mut end) = (None, None);
    loop {
        match written.kind() {
            "type_identifier" => {
                return Some((start.unwrap_or(written), end.unwrap_or(written), written));
            }
            "qualified_identifier" => start = start.or(Some(written)),
            "template_type" => {
                start = start.or(Some(written));
                end = end.or(Some(written));
            }
            "struct_specifier" | "class_specifier" | "union_specifier" => {}
            _ => return None,
        }
        written = written.child_by_field_name("name")?;
    }
}

fn fills(parent: Node<'_>, field: &str, child: Node<'_>) -> bool {
    parent.child_by_field_name(field) == Some(child)
}

// constraint: a name that declares, labels, qualifies, calls or sits in a directive is not a use of a value; a call's
// callee is a call site already
fn is_used(name: Node<'_>, parent: Node<'_>) -> bool {
    match parent.kind() {
        "declaration"
        | "init_declarator"
        | "parameter_declaration"
        | "optional_parameter_declaration"
        | "variadic_parameter_declaration"
        | "pointer_declarator"
        | "array_declarator"
        | "function_declarator"
        | "for_range_loop" => !fills(parent, "declarator", name),
        "lambda_capture_initializer" => !fills(parent, "left", name),
        "enumerator" => !fills(parent, "name", name),
        "call_expression" => !fills(parent, "function", name),
        "reference_declarator"
        | "structured_binding_declarator"
        | "parenthesized_declarator"
        | "attributed_declarator"
        | "variadic_declarator"
        | "destructor_name"
        | "qualified_identifier"
        | "template_function"
        | "template_method"
        | "dependent_name"
        | "goto_statement"
        | "labeled_statement"
        | "lambda_capture_specifier"
        | "attribute"
        | "using_declaration"
        | "namespace_alias_definition"
        | "field_declaration" => false,
        kind => !kind.starts_with("preproc_"),
    }
}
