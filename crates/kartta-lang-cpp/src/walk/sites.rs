use std::collections::HashMap;

use kartta_lang::tree::text;
use kartta_lang::{Access, Receiver, UseRef};
use tree_sitter::Node;

use super::data::data_declarator;
use super::{Replayer, line};

/// The names the bodies around the walk declare, each visible from its declaration to the end of the block that
/// declares it, the latest of a name shadowing the ones before it.
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
            "field_expression" => self.record_member(node),
            "field_initializer" => self.record_initialized_field(node),
            "initializer_pair" => self.record_designated_field(node),
            _ => {}
        }
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
            .and_then(|written| self.owner_named(written));
        let mut cursor = declaration.walk();
        for declarator in declaration.children_by_field_name("declarator", &mut cursor) {
            self.declare_names(declarator, owner, end);
        }
    }

    fn declare_names(&mut self, declarator: Node<'_>, owner: Option<&'s str>, end: usize) {
        if let Some((name, pointer)) = data_declarator(declarator, self.source) {
            let holds = match (owner, pointer) {
                (Some(owner), None) => Holds::Value(owner),
                (Some(owner), Some(_)) => Holds::Pointer(owner),
                (None, _) => Holds::Unknown,
            };
            if let Some(name) = self.borrowed(name) {
                self.locals.declare(name, end, holds);
            }
            return;
        }
        match declarator.kind() {
            "structured_binding_declarator" => {
                let source = self.source;
                let mut cursor = declarator.walk();
                let names = declarator
                    .named_children(&mut cursor)
                    .filter_map(|name| borrowed(source, name));
                for name in names {
                    self.locals.declare(name, end, Holds::Unknown);
                }
            }
            "init_declarator" => {
                if let Some(inner) = declarator.child_by_field_name("declarator") {
                    self.declare_names(inner, owner, end);
                }
            }
            "reference_declarator"
            | "variadic_declarator"
            | "attributed_declarator"
            | "parenthesized_declarator" => {
                if let Some(inner) = declarator.named_child(0) {
                    self.declare_names(inner, None, end);
                }
            }
            _ => {}
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

    fn owner_named(&self, written: Node<'_>) -> Option<&'s str> {
        owner_type(written)
            .and_then(|owner| self.borrowed(owner))
            .filter(|owner| !self.is_template_parameter(owner))
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
        if let Some(field) = node
            .named_child(0)
            .filter(|field| field.kind() == "field_identifier")
        {
            let name = text(field, self.source);
            self.push_site(field, (name, Receiver::Own, Access::Write));
        }
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
        self.owner_named(written?)
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
        self.raw.uses.push(UseRef {
            site_of: self.caller(),
            name,
            receiver,
            access,
            line: line(at.start_position().row),
        });
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

fn borrowed<'s>(source: &'s [u8], node: Node<'_>) -> Option<&'s str> {
    std::str::from_utf8(&source[node.byte_range()]).ok()
}

fn assignment_access(assignment: Node<'_>) -> Access {
    let plain = assignment
        .child_by_field_name("operator")
        .is_none_or(|operator| operator.kind() == "=");
    if plain { Access::Write } else { Access::Update }
}

fn owner_type(written: Node<'_>) -> Option<Node<'_>> {
    match written.kind() {
        "type_identifier" => Some(written),
        "qualified_identifier"
        | "template_type"
        | "struct_specifier"
        | "class_specifier"
        | "union_specifier" => written.child_by_field_name("name").and_then(owner_type),
        _ => None,
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
