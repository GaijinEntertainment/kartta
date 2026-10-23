use kartta_lang::tree::named_child_nodes;
use tree_sitter::Node;

/// What an identifier is where it stands: a binding of a local, a read that names a local or calls a method, or a name
/// that is neither.
pub(crate) enum Role {
    Binding,
    Read,
    Name,
}

// constraint: Ruby reads a bare name as a local where a binding of it comes before it in the source, in its own scope
// or in a scope a block sees; a `def` and the body of a class, a module or a `class << x` open a scope that sees
// nothing around it, and a block or a lambda one that sees the scopes around it
#[derive(Default)]
pub(crate) struct Locals<'s> {
    names: Vec<&'s [u8]>,
    frames: Vec<Frame>,
}

#[derive(Clone, Copy)]
struct Frame {
    end: usize,
    first: usize,
    visible: usize,
    block: bool,
}

impl<'s> Locals<'s> {
    pub(crate) fn leave(&mut self, start: usize) {
        while let Some(frame) = self
            .frames
            .last()
            .copied()
            .filter(|frame| frame.end <= start)
        {
            self.names.truncate(frame.first);
            self.frames.pop();
        }
    }

    pub(crate) fn enter(&mut self, node: Node<'_>, source: &'s [u8]) {
        let (visible, block) = match node.kind() {
            "method" | "singleton_method" => (self.names.len(), false),
            "body_statement"
                if node.parent().is_some_and(|parent| {
                    matches!(parent.kind(), "class" | "module" | "singleton_class")
                }) =>
            {
                (self.names.len(), false)
            }
            "block" | "do_block" | "lambda" => (self.visible(), true),
            "keyword_pattern" => return self.bind_shorthand_key(node, source),
            "binary" => return self.bind_named_groups(node, source),
            _ => return,
        };
        self.frames.push(Frame {
            end: node.end_byte(),
            first: self.names.len(),
            visible,
            block,
        });
    }

    pub(crate) fn bind(&mut self, name: &'s [u8]) {
        self.names.push(name);
    }

    // constraint: inside a block or a lambda, `it` and `_1` to `_9` name its implicit parameters; Ruby rejects them in
    // one that declares parameters, and outside every block `it` is a method call
    pub(crate) fn is_local(&self, name: &[u8]) -> bool {
        self.names[self.visible()..].contains(&name)
            || matches!(name, b"it" | [b'_', b'1'..=b'9'])
                && self.frames.last().is_some_and(|frame| frame.block)
    }

    fn visible(&self) -> usize {
        self.frames.last().map_or(0, |frame| frame.visible)
    }

    // constraint: `in {name:}` binds `name`
    fn bind_shorthand_key(&mut self, pattern: Node<'_>, source: &'s [u8]) {
        if pattern.child_by_field_name("value").is_none()
            && let Some(key) = pattern
                .child_by_field_name("key")
                .filter(|key| key.kind() == "hash_key_symbol")
        {
            self.bind(&source[key.byte_range()]);
        }
    }

    // constraint: `/(?<name>.)/ =~ text` binds each named group, where the regexp is a literal without interpolation
    // on the left; an escaped `(` and one inside a character class, `[(?<x>)]`, open no group
    fn bind_named_groups(&mut self, binary: Node<'_>, source: &'s [u8]) {
        let Some(regex) = binary
            .child_by_field_name("left")
            .filter(|left| left.kind() == "regex")
        else {
            return;
        };
        if binary
            .child_by_field_name("operator")
            .is_none_or(|operator| operator.kind() != "=~")
            || named_child_nodes(regex).any(|part| part.kind() == "interpolation")
        {
            return;
        }
        let mut rest = &source[regex.byte_range()];
        let mut class_depth = 0_usize;
        while let Some((&byte, after)) = rest.split_first() {
            match byte {
                b'[' => class_depth += 1,
                b']' => class_depth = class_depth.saturating_sub(1),
                b'(' if class_depth == 0 => self.names.extend(group_name(after)),
                _ => {}
            }
            rest = if byte == b'\\' {
                after.get(1..).unwrap_or_default()
            } else {
                after
            };
        }
    }
}

/// The name of the group whose `(` comes right before `group`, where it is a named group, `(?<name>...)`.
fn group_name(group: &[u8]) -> Option<&[u8]> {
    let rest = group.strip_prefix(b"?<")?;
    let length = rest
        .iter()
        .take_while(|byte| byte.is_ascii_alphanumeric() || **byte == b'_')
        .count();
    let name = &rest[..length];
    (name.first().is_some_and(|first| !first.is_ascii_digit()) && rest.get(length) == Some(&b'>'))
        .then_some(name)
}

// constraint: the grammar parses the keywords `__FILE__`, `__LINE__` and `__ENCODING__` as names
pub(crate) const fn is_keyword(name: &[u8]) -> bool {
    matches!(name, b"__FILE__" | b"__LINE__" | b"__ENCODING__")
}

// constraint: the grammar parses a bare name as an `identifier` everywhere a value stands, and in the binding and the
// naming positions below
pub(crate) fn role(identifier: Node<'_>) -> Role {
    let Some(parent) = identifier.parent() else {
        return Role::Read;
    };
    let in_field = |field| {
        parent
            .child_by_field_name(field)
            .is_some_and(|child| child.id() == identifier.id())
    };
    match parent.kind() {
        "method_parameters"
        | "lambda_parameters"
        | "block_parameters"
        | "destructured_parameter"
        | "left_assignment_list"
        | "destructured_left_assignment"
        | "rest_assignment"
        | "exception_variable"
        | "for"
        | "array_pattern"
        | "find_pattern"
        | "alternative_pattern"
        | "parenthesized_pattern"
        | "as_pattern"
        | "keyword_pattern"
        | "in_clause" => Role::Binding,
        "optional_parameter"
        | "keyword_parameter"
        | "splat_parameter"
        | "hash_splat_parameter"
        | "block_parameter"
            if in_field("name") =>
        {
            Role::Binding
        }
        "assignment" | "operator_assignment" if in_field("left") => Role::Binding,
        "match_pattern" | "test_pattern" if in_field("pattern") => Role::Binding,
        "call" if in_field("method") => Role::Name,
        "method" | "singleton_method" if !in_field("body") => Role::Name,
        "alias" | "undef" | "setter" | "variable_reference_pattern" => Role::Name,
        _ => Role::Read,
    }
}
