use std::collections::HashMap;
use std::ops::Range;

use kartta_lang::tree::{line, named_child_nodes, signature_from, text};
use kartta_lang::{
    CallRef, FILE_SCOPE, LocalCall, RawExtraction, Receiver, SymbolId, SymbolKind, UseRef,
};
use tree_sitter::Node;

use crate::bindings::MethodKind;
use crate::owner::{enclosing_type, in_singleton_class};
use crate::walk::{Emitter, declared, plain_string, visibility_exports};

/// A method `let` or `subject` defines, with the bytes of its example group.
pub(crate) struct GroupMethod<'s> {
    name: &'s [u8],
    id: SymbolId,
    group: Range<usize>,
}

/// A shared group an example group includes: its name as [`shared_group_name`] spells it, and the bytes that reach it.
pub(crate) struct IncludedGroup {
    name: String,
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
            "include_context" | "include_examples" => {
                self.included_group(call, enclosing_group(call));
            }
            "it_behaves_like" | "it_should_behave_like" => {
                if let Some(block) = call.child_by_field_name("block") {
                    self.included_group(call, block);
                }
            }
            _ if is_example_group_or_example(callee) => self.filtered_block(call),
            _ => {}
        }
    }

    /// Reads a call with a receiver that names a metadata filter: `RSpec.describe` and `config.include_context`.
    pub(crate) fn filter_macro(&mut self, call: Node<'_>, callee: &str, receiver: Node<'_>) {
        if callee == "include_context" {
            self.group_a_filter_includes(call);
        } else if is_example_group_or_example(callee)
            && &self.source[receiver.byte_range()] == b"RSpec"
        {
            self.filtered_block(call);
        }
    }

    // constraint: RSpec applies `config.include_context` to a group or an example whose metadata matches any of its
    // filters when the group is made, ahead of its block, and a nested group is a subclass that inherits it
    // (`configuration.rb:1475`, `:533`, `:1571`, `:1587`, `example_group.rb:453`, `metadata_filter.rb:12`), so the
    // block's own methods replace the included ones whatever their order. A call passes its metadata after the
    // description (`metadata.rb:80`)
    fn filtered_block(&mut self, call: Node<'_>) {
        let Some(block) = call.child_by_field_name("block") else {
            return;
        };
        for filter in filters(call, self.source) {
            self.included_groups.push(IncludedGroup {
                name: format!("@{filter}"),
                group: block.byte_range(),
            });
        }
    }

    // constraint: the name and the group's metadata stand in two files, so the file records what the filter pastes in
    // for the graph to join: the group alone, to find this file from the group, and the group with each filter
    fn group_a_filter_includes(&mut self, call: Node<'_>) {
        let Some(group) = shared_group_name(call, self.source) else {
            return;
        };
        let line = line(call.start_position().row);
        let mut filtered = filters(call, self.source)
            .map(|filter| file_scope_read(format!("{group}@{filter}"), line))
            .peekable();
        if filtered.peek().is_some() {
            self.raw.uses.extend(filtered);
            self.raw.uses.push(file_scope_read(group, line));
        }
    }

    // constraint: RSpec runs a shared group's block on the group that includes it, in that group for `include_context`
    // and `include_examples` (`example_group.rb:350`, `:360`) and in a group of its own, with the block the call
    // passes, for `it_behaves_like` (`example_group.rb:329`, `shared_example_group.rb:38`), so that block alone reads
    // its lets
    fn included_group(&mut self, call: Node<'_>, group: Node<'_>) {
        if let Some(name) = shared_group_name(call, self.source) {
            self.included_groups.push(IncludedGroup {
                name,
                group: group.byte_range(),
            });
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
        let group = enclosing_group(call);
        let mut definition = declared(name.clone(), name, SymbolKind::Function, call);
        definition.signature = signature_from(call.start_byte(), arguments, source);
        definition
            .value_types
            .extend(shared_group_of_every_file(group, source));
        let id = self.push(definition);
        self.raw.local_symbols.push(id);
        self.open_method(call, id);
        self.group_methods.push(GroupMethod {
            name: spelled.strip_prefix(b":").unwrap_or(spelled),
            id,
            group: group.byte_range(),
        });
    }

    // constraint: a method of an example group takes no argument and no block, so a call that passes one is a DSL's
    // method of the name
    pub(crate) fn plain_call(&mut self, node: Node<'_>) {
        self.plain_calls
            .push((self.raw.calls.len(), node.start_byte()));
    }

    // constraint: a method of an example group is reached from the group and the groups nested in it, wherever in them
    // it is defined, so a call binds the innermost one around it, and of two in one group the later, which replaces the
    // earlier. A call at file scope keeps its name, which is all the graph keeps of it. A group's own method stands
    // ahead of the groups it includes, whose methods the graph alone can read; of those a later include in one group
    // replaces an earlier one, since each runs its block on the group
    pub(crate) fn bind_group_methods(&mut self) {
        let mut named: HashMap<&[u8], Vec<&GroupMethod<'_>>> = HashMap::new();
        for method in &self.group_methods {
            named.entry(method.name).or_default().push(method);
        }
        if named.is_empty() && self.included_groups.is_empty() {
            return;
        }
        self.included_groups.reverse();
        self.included_groups
            .sort_by_key(|included| included.group.len());
        let included = &self.included_groups;
        let RawExtraction {
            calls,
            local_calls,
            uses,
            ..
        } = &mut self.raw;
        let mut plain = self.plain_calls.iter().copied().peekable();
        let mut at = 0;
        calls.retain_mut(|call| {
            let byte = plain.next_if(|&(call, _)| call == at).map(|(_, byte)| byte);
            at += 1;
            let Some(byte) = byte else {
                return true;
            };
            let innermost = named.get(call.callee.as_bytes()).and_then(|methods| {
                (methods.iter().rev())
                    .filter(|method| method.group.contains(&byte))
                    .min_by_key(|method| method.group.len())
            });
            match innermost {
                Some(method) if call.site_of != FILE_SCOPE => {
                    local_calls.push(LocalCall::new(call.site_of, method.id, call.line));
                    false
                }
                Some(_) => true,
                None => {
                    read_included_groups(call, byte, included, uses);
                    true
                }
            }
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

/// Marks a bare `call` at `byte` with the groups in `included` around it, nearest first, or at file scope, where a call
/// keeps its name alone, records a use of what each would define under the name.
fn read_included_groups(
    call: &mut CallRef,
    byte: usize,
    included: &[IncludedGroup],
    uses: &mut Vec<UseRef>,
) {
    if call.receiver.is_some() {
        return;
    }
    let mut groups = (included.iter())
        .filter(|included| included.group.contains(&byte))
        .map(|included| included.name.as_str())
        .peekable();
    if groups.peek().is_none() {
        return;
    }
    if call.site_of == FILE_SCOPE {
        uses.extend(
            groups.map(|group| file_scope_read(format!("{group}#{}", call.callee), call.line)),
        );
    } else {
        call.receiver = Some(format!("+{}", groups.collect::<Vec<_>>().join("\n")));
    }
}

// constraint: a shared group's name is a key of a hash, so a string and a symbol of one spelling are two names
// (`shared_example_group.rb:171`, `:176`); a `Module` or a computed name is not read
/// The name of the shared group `call` defines or includes, its first argument as a string in single quotes or a symbol
/// with its `:`; `None` for any other argument.
fn shared_group_name(call: Node<'_>, source: &[u8]) -> Option<String> {
    let name = named_child_nodes(call.child_by_field_name("arguments")?).next()?;
    match name.kind() {
        "simple_symbol" => Some(text(name, source)),
        "string" => plain_string(name, source)
            .filter(|content| !content.contains('\n'))
            .map(|content| format!("'{content}'")),
        _ => None,
    }
}

// constraint: `RSpec.shared_context` registers its group for every file, and so does a bare one outside every block,
// while a bare one in a group's block registers it for that group and the groups nested in it
// (`shared_example_group.rb:98`, `:111`, `:174`), so another file reaches only the first two
/// The names of the symbols `call` passes after its first argument, a description or a shared group's name.
fn filters<'a>(call: Node<'a>, source: &'a [u8]) -> impl Iterator<Item = String> + 'a {
    (call.child_by_field_name("arguments").into_iter())
        .flat_map(named_child_nodes)
        .skip(1)
        .filter(|argument| argument.kind() == "simple_symbol")
        .map(|filter| text(filter, source).trim_start_matches(':').to_owned())
}

const fn file_scope_read(name: String, line: u32) -> UseRef {
    UseRef::new(FILE_SCOPE, name, Receiver::Bare, line)
}

// constraint: `example_group.rb:158-198` and `:282-308` define these, and `RSpec.describe` is an alias of the group's
fn is_example_group_or_example(callee: &str) -> bool {
    matches!(
        callee,
        "example_group"
            | "describe"
            | "context"
            | "xdescribe"
            | "xcontext"
            | "fdescribe"
            | "fcontext"
            | "example"
            | "it"
            | "specify"
            | "focus"
            | "fexample"
            | "fit"
            | "fspecify"
            | "xexample"
            | "xit"
            | "xspecify"
            | "skip"
            | "pending"
    )
}

/// The name of the shared group `block` is the block of, where every file reaches it.
fn shared_group_of_every_file(block: Node<'_>, source: &[u8]) -> Option<String> {
    let definition = block.parent().filter(|parent| parent.kind() == "call")?;
    let method = definition.child_by_field_name("method")?;
    if !matches!(
        &source[method.byte_range()],
        b"shared_context" | b"shared_examples" | b"shared_examples_for"
    ) {
        return None;
    }
    let of_every_file = definition.child_by_field_name("receiver").map_or_else(
        || !in_a_block(definition),
        |receiver| &source[receiver.byte_range()] == b"RSpec",
    );
    if of_every_file {
        shared_group_name(definition, source)
    } else {
        None
    }
}

fn in_a_block(node: Node<'_>) -> bool {
    std::iter::successors(node.parent(), Node::parent)
        .any(|parent| matches!(parent.kind(), "block" | "do_block"))
}

/// The example group that holds `call`: the block or the type around it, else the whole file.
fn enclosing_group(call: Node<'_>) -> Node<'_> {
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
    group
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
