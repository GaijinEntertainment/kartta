use kartta_lang::SymbolId;
use kartta_lang::tree::{child_nodes, named_child_nodes, text};
use tree_sitter::Node;

use super::Emitter;
use super::blocks::{CallForm, PathSegment};
use super::calls::{Called, MacroName, Target, value_receiver};

// constraint: the names a closure written in the arguments binds hide a bare call of the name, as a parameter does, to
// the end of the token tree the closure is written in, since the tokens do not tell where its body ends; what is found
// is recorded once the arguments are read, and nothing of a template; `quiet` marks a tree whose words read no value,
// one of an item the arguments declare, of a turbofish or of a cast's type
struct Reading<'a, 't> {
    bound: Vec<&'a [u8]>,
    found: Vec<(Node<'t>, Found)>,
    template: bool,
    quiet: bool,
}

// constraint: a read carries the path it is written through, `Kind` of `Kind::A`, or none for a bare name
enum Found {
    Call(Called),
    Macro(MacroName),
    Read(Option<String>),
}

// constraint: an item the arguments declare, `struct Pair { first: Point }`, names itself and its types, no value the
// site reads: a `const` or a `static` up to its `=`, whose value is read, and any other up to its `;` or through its
// first `{` tree
#[derive(Clone, Copy, PartialEq, Eq)]
enum Item {
    No,
    Value,
    Body,
}

impl Item {
    // constraint: `fn` hands its declaration to `Declared`, and a `{` tree ends a body once it is read; `*const T` and
    // `&'static T` name a type and `const { .. }` is a block, no item
    fn after(self, tokens: &[Node<'_>], at: usize) -> Self {
        let token = tokens[at];
        match token.kind() {
            "const" | "static"
                if !at
                    .checked_sub(1)
                    .is_some_and(|before| matches!(tokens[before].kind(), "*" | "'"))
                    && tokens
                        .get(at + 1)
                        .is_none_or(|next| next.kind() != "token_tree") =>
            {
                Self::Value
            }
            "struct" | "enum" | "trait" | "impl" | "mod" | "type" | "use" => Self::Body,
            "=" if self == Self::Value => Self::No,
            "token_tree" if self == Self::Body && delimiter(token) == Some("{") => Self::No,
            "fn" | ";" => Self::No,
            _ => self,
        }
    }
}

// constraint: a `let`'s names bind once its initializer ends, at its `;`, to the end of the token tree, and an `if
// let`'s, a `while let`'s or a `for`'s from the `&&` that chains a condition to it, or else the block it opens, to the
// end of that block, since the initializer and the iterated value are read before the binding and an `else` after it;
// `scope` is the length `bound` takes back once the block is read
#[derive(Default)]
struct Pending<'a> {
    names: Vec<&'a [u8]>,
    at_block: bool,
    scope: Option<usize>,
}

impl<'a> Pending<'a> {
    fn settle(&mut self, token: Node<'_>, bound: &mut Vec<&'a [u8]>) -> Option<usize> {
        if !self.at_block {
            if token.kind() == ";" {
                bound.append(&mut self.names);
            }
            return None;
        }
        let block = token.kind() == "token_tree" && delimiter(token) == Some("{");
        if !block && token.kind() != "&&" {
            return None;
        }
        let scope = *self.scope.get_or_insert(bound.len());
        bound.append(&mut self.names);
        if !block {
            return None;
        }
        self.at_block = false;
        self.scope = None;
        Some(scope)
    }
}

// constraint: a `fn` the arguments declare, `quote! { fn f(x: u8) { g(x) } }`, is code the macro generates, so its
// parameter list and its body hold no call or read of the site
#[derive(Clone, Copy, PartialEq, Eq)]
enum Declared {
    No,
    Parameters,
    Body,
}

impl Declared {
    // constraint: whether the tree at `at` is the parameter list or the body of the declared `fn`, which the reading
    // skips; a block between `<` or `,` and `>` or `,` is a const generic argument of a type, `-> Foo<{ N }>`
    fn passes(&mut self, tokens: &[Node<'_>], at: usize) -> bool {
        let generic = at
            .checked_sub(1)
            .is_some_and(|before| matches!(tokens[before].kind(), "<" | ","))
            && tokens
                .get(at + 1)
                .is_some_and(|next| matches!(next.kind(), ">" | ">>" | ","));
        match (*self, delimiter(tokens[at])) {
            (Self::Parameters, Some("(")) => *self = Self::Body,
            (Self::Body, Some("{")) if !generic => *self = Self::No,
            _ => return false,
        }
        true
    }
}

impl<'a> Emitter<'a> {
    // constraint: tree-sitter-rust parses a macro's arguments as a token tree, the tokens and the token trees nested in
    // them, so they hold no call expression and no read; a call is read from the tokens as a name, a path or a member
    // before a `(` tree, a nested macro invocation as a name before `!`, and a read as a word an expression would read
    pub(super) fn macro_arguments(&mut self, invocation: Node<'_>, site_of: SymbolId) {
        let Some(arguments) =
            named_child_nodes(invocation).find(|child| child.kind() == "token_tree")
        else {
            return;
        };
        let mut reading = Reading {
            bound: Vec::new(),
            found: Vec::new(),
            template: false,
            quiet: false,
        };
        let name = invocation
            .child_by_field_name("macro")
            .map(|name| name.child_by_field_name("name").unwrap_or(name));
        let name = name.map_or(&[][..], |name| &self.source[name.byte_range()]);
        self.read_arguments(name, arguments, &mut reading);
        if reading.template {
            return;
        }
        for (name, found) in reading.found {
            match found {
                Found::Call(called) => self.record_call(name, site_of, called),
                Found::Macro(macro_name) => self.record_macro_call(name, site_of, macro_name),
                Found::Read(None) => self.bare_read(name),
                Found::Read(Some(path)) => self.path_read(path, name),
            }
        }
    }

    // constraint: a `macro_rules! matches` the file defines before the invocation, in the block or module around it,
    // is in scope by text order and takes the name from the standard macro, and its arguments are read as any macro's
    fn read_arguments<'t>(&self, name: &[u8], tree: Node<'t>, reading: &mut Reading<'a, 't>) {
        if name == b"matches" && tree.start_byte() >= self.matches_scope_end {
            self.matches_arguments(tree, reading);
        } else {
            self.read_tokens(tree, reading);
        }
    }

    // constraint: `matches!(value, pattern if guard)` reads its value and its guard, and its pattern names
    // constructors, which are no call, and binds names the guard reads; the pattern runs from the first `,` to the
    // first `if` after it, since the value can hold an `if` of its own, and a closure's parameters there, `|x, y|`,
    // hold a `,` of their own
    fn matches_arguments<'t>(&self, tree: Node<'t>, reading: &mut Reading<'a, 't>) {
        let depth = reading.bound.len();
        let tokens: Vec<Node<'t>> = child_nodes(tree).collect();
        let mut in_pattern = false;
        let mut guarded = false;
        let mut pattern = 0;
        let mut typed = 0;
        let mut at = 0;
        while let Some(&token) = tokens.get(at) {
            at += 1;
            match token.kind() {
                "|" if !in_pattern && opens_closure(&tokens[..at - 1], self.source) => {
                    at = self.bind_closure(&tokens, at, reading);
                }
                "," if !guarded && !in_pattern => (in_pattern, pattern) = (true, at),
                "if" if in_pattern => {
                    self.bind_arm(&tokens[pattern..at - 1], reading);
                    (in_pattern, guarded) = (false, true);
                }
                "::" if at > typed => typed = generics_end(&tokens, at).unwrap_or(typed),
                "as" if at > typed => typed = cast_end(&tokens, at),
                "identifier" if !in_pattern && !reading.quiet && at > typed => {
                    self.push_read(&tokens, at - 1, reading);
                }
                "token_tree" if !in_pattern => self.token_tree(&tokens, at - 1, reading),
                _ => {}
            }
        }
        reading.bound.truncate(depth);
    }

    // constraint: an interpolation, `#name` or `#(..)*`, makes the arguments a template of generated code,
    // `quote! { #name(x) }`, whose calls and reads are the generated code's and none of the site
    fn read_tokens<'t>(&self, tree: Node<'t>, reading: &mut Reading<'a, 't>) {
        if tree.named_child_count() == 0 {
            return;
        }
        let depth = reading.bound.len();
        let tokens: Vec<Node<'t>> = child_nodes(tree).collect();
        let mut declared = Declared::No;
        let mut item = Item::No;
        let mut pending = Pending::default();
        let mut arm = 0;
        let mut typed = 0;
        let mut at = 0;
        while let Some(&token) = tokens.get(at) {
            at += 1;
            let scoped = pending.settle(token, &mut reading.bound);
            let quiet =
                reading.quiet || item != Item::No || declared != Declared::No || at <= typed;
            item = item.after(&tokens, at - 1);
            match token.kind() {
                "#" if tokens.get(at).is_some_and(|next| {
                    is_word(*next) || (next.kind() == "token_tree" && delimiter(*next) == Some("("))
                }) =>
                {
                    reading.template = true;
                }
                "fn" if tokens.get(at).is_some_and(|name| is_word(*name)) => {
                    declared = Declared::Parameters;
                }
                ";" => declared = Declared::No,
                "," => arm = at,
                "::" if at > typed => typed = generics_end(&tokens, at).unwrap_or(typed),
                "as" if at > typed => typed = cast_end(&tokens, at),
                "|" if opens_closure(&tokens[..at - 1], self.source) => {
                    at = self.bind_closure(&tokens, at, reading);
                }
                "let" | "for" => at = self.bind_statement(&tokens, at, &mut pending),
                "if" if is_guard(&tokens[at..]) => {
                    self.bind_arm(&tokens[arm.min(at - 1)..at - 1], reading);
                    arm = at - 1;
                }
                "=>" => self.bind_arm(&tokens[arm.min(at - 1)..at - 1], reading),
                "identifier" if !quiet => self.push_read(&tokens, at - 1, reading),
                "token_tree" if !declared.passes(&tokens, at - 1) => {
                    let around = std::mem::replace(&mut reading.quiet, quiet);
                    self.token_tree(&tokens, at - 1, reading);
                    reading.quiet = around;
                }
                _ => {}
            }
            if let Some(before) = scoped {
                reading.bound.truncate(before);
            }
            if token.kind() == "token_tree" && at >= 2 && tokens[at - 2].kind() == "=>" {
                arm = at;
            }
            if reading.template {
                break;
            }
        }
        reading.bound.truncate(depth);
    }

    // constraint: a `let` pattern runs to its `=`, a `for` pattern to its `in`, each before its `:` type, and neither
    // is read for calls; the names wait in `pending` for the end of the initializer or the iterated value
    fn bind_statement(&self, tokens: &[Node<'_>], at: usize, pending: &mut Pending<'a>) -> usize {
        let source = self.source;
        let keyword = tokens[at - 1].kind();
        pending.at_block = keyword == "for"
            || at
                .checked_sub(2)
                .is_some_and(|before| matches!(tokens[before].kind(), "if" | "while" | "&&"));
        let ends = |token: &Node<'_>| match token.kind() {
            "=" | ";" => true,
            "identifier" => keyword == "for" && &source[token.byte_range()] == b"in",
            _ => false,
        };
        let end = tokens[at..]
            .iter()
            .position(ends)
            .map_or(tokens.len(), |end| at + end);
        let pattern = tokens[at..end]
            .iter()
            .copied()
            .take_while(|token| token.kind() != ":");
        bind_names(pattern, &mut |name| {
            pending.names.push(&source[name.byte_range()]);
        });
        end
    }

    // constraint: a match arm's pattern runs from the `,` or the block that ends the arm before it to its guard or
    // its `=>`, and binds its names to the end of the token tree, as a closure's parameters do; a guard binds them
    // at its `if`, since it reads them. A pattern reads no value, so the reads found in it are dropped
    fn bind_arm(&self, arm: &[Node<'_>], reading: &mut Reading<'a, '_>) {
        let Some(first) = arm.first().filter(|first| first.kind() != "if") else {
            return;
        };
        let pattern = arm.iter().copied().take_while(|token| token.kind() != "if");
        bind_names(pattern, &mut |name| {
            reading.bound.push(&self.source[name.byte_range()]);
        });
        let from = first.start_byte();
        let in_pattern = reading
            .found
            .iter()
            .rposition(|(node, _)| node.start_byte() < from)
            .map_or(0, |at| at + 1);
        reading
            .found
            .extract_if(in_pattern.., |(_, found)| matches!(found, Found::Read(_)))
            .for_each(drop);
    }

    // constraint: a word is read where an expression would read it: not before a `(` tree, `!`, `::`, `:` or `=`, as a
    // callee, a macro, a path's segment, a field's label or a named argument; not after `.`, as a field; and not before
    // a `{` tree, as the struct its fields build, `Point { x }`, unless the word ends the condition of an `if`, a
    // `while` or a `match` or the value a `for` iterates, `if READY { .. }`. A path's last segment is a read through
    // the path. A bare word is one only when capitalised, as a constant, a static or a unit struct is, since a macro
    // takes a lowercase word by name as often as a value, `cov_mark::check!(a_test)`, and a name a pattern binds is
    // none
    fn push_read<'t>(&self, tokens: &[Node<'t>], at: usize, reading: &mut Reading<'a, 't>) {
        let (token, before) = (tokens[at], &tokens[..at]);
        let names_no_value = tokens.get(at + 1).is_some_and(|next| match next.kind() {
            "!" | "::" | ":" | "=" => true,
            "token_tree" => match delimiter(*next) {
                Some("(") => true,
                Some("{") => !in_condition(before, self.source),
                _ => false,
            },
            _ => false,
        });
        if names_no_value {
            return;
        }
        let found = match before.last().map(Node::kind) {
            Some(".") => return,
            Some("::") => {
                let path = path_before(before);
                if path.rooted || path.segments.is_empty() {
                    return;
                }
                Found::Read(Some(self.spelled(&path)))
            }
            _ => {
                let spelled = &self.source[token.byte_range()];
                if !spelled.first().is_some_and(u8::is_ascii_uppercase)
                    || self.is_bound(spelled)
                    || reading.bound.contains(&spelled)
                {
                    return;
                }
                Found::Read(None)
            }
        };
        reading.found.push((token, found));
    }

    // constraint: a nested macro is a template, or not, of its own, so a template drops what its own arguments hold
    // and nothing of the macro around it
    fn nested_arguments<'t>(&self, name: &[u8], tree: Node<'t>, reading: &mut Reading<'a, 't>) {
        let found = reading.found.len();
        let around = std::mem::replace(&mut reading.template, false);
        self.read_arguments(name, tree, reading);
        if reading.template {
            reading.found.truncate(found);
        }
        reading.template = around;
    }

    // constraint: a tree after `#` or `#!` is an attribute, `#[derive(Debug)]`, and holds no call or read of the site;
    // a `!` after anything but a name is a negation, `!(a || b)`; a `(` tree before `=`, a guard's `if` or an
    // unguarded arm's `=>` ends a pattern, `if let Some(x) =`, `Some(x) if ready(x) =>` or `Kind::A(x) =>`, which
    // names constructors and holds no call or read, since no expression continues with `if`; before a guarded arm's
    // `=>` it ends the guard, a call
    fn token_tree<'t>(&self, tokens: &[Node<'t>], at: usize, reading: &mut Reading<'a, 't>) {
        let (tree, before) = (tokens[at], &tokens[..at]);
        match before.last().map(Node::kind) {
            Some("#") => return,
            Some("!") => {
                let name = &before[..at - 1];
                if name.last().is_some_and(|last| last.kind() == "#") {
                    return;
                }
                if let Some(found) = self.token_macro(name) {
                    let name = &self.source[found.0.byte_range()];
                    reading.found.push(found);
                    return self.nested_arguments(name, tree, reading);
                }
            }
            _ if delimiter(tree) == Some("(") => {
                let ends_pattern = tokens.get(at + 1).is_some_and(|next| match next.kind() {
                    "=" | "if" => true,
                    "=>" => !before
                        .iter()
                        .rev()
                        .take_while(|token| !matches!(token.kind(), "," | "=>"))
                        .any(|token| token.kind() == "if"),
                    _ => false,
                });
                if ends_pattern {
                    return;
                }
                if let Some(found) = self.token_call(before, reading) {
                    reading.found.push(found);
                }
            }
            _ => {}
        }
        self.read_tokens(tree, reading);
    }

    fn token_macro<'t>(&self, before: &[Node<'t>]) -> Option<(Node<'t>, Found)> {
        let (&name, rest) = before
            .split_last()
            .filter(|(name, _)| is_callee(**name, self.source))?;
        let callee = text(name, self.source);
        let path = path_before(rest);
        let name_of = if path.segments.is_empty() && !path.rooted {
            MacroName::Bare(callee)
        } else {
            let mut written = self.spelled(&path);
            if !path.segments.is_empty() {
                written.push_str("::");
            }
            written.push_str(&callee);
            MacroName::Scoped(callee, Some(written))
        };
        Some((name, Found::Macro(name_of)))
    }

    fn token_call<'t>(
        &self,
        before: &[Node<'t>],
        reading: &Reading<'_, '_>,
    ) -> Option<(Node<'t>, Found)> {
        let (name, rest) = called_name(before, self.source)?;
        let callee = text(name, self.source);
        let found = match rest
            .split_last()
            .map(|(last, operand)| (last.kind(), operand))
        {
            Some((".", operand)) => self.member_called(callee, operand, reading),
            Some(("::", _)) => self.path_called(callee, &path_before(rest)),
            _ => {
                let spelled = &self.source[name.byte_range()];
                if self.is_bound(spelled) || reading.bound.contains(&spelled) {
                    return None;
                }
                Called {
                    target: Target::new(callee, None),
                    written: None,
                    form: Some(CallForm::Plain),
                }
            }
        };
        Some((name, Found::Call(found)))
    }

    // constraint: a member call takes a named operand, `self` as the type of the impl or trait around it, and anything
    // else, a call, a field, a path or a literal, as the empty receiver, as a member call the syntax tree spells does;
    // a name a closure in the arguments binds is no parameter of the function around the macro
    fn member_called(
        &self,
        callee: String,
        operand: &[Node<'_>],
        reading: &Reading<'_, '_>,
    ) -> Called {
        let chained = operand
            .len()
            .checked_sub(2)
            .is_some_and(|at| matches!(operand[at].kind(), "." | "::"));
        let target = match operand.last() {
            Some(&value) if !chained && value.kind() == "identifier" => {
                let spelled = &self.source[value.byte_range()];
                if reading.bound.contains(&spelled) {
                    Target::new(callee, Some(value_receiver(spelled)))
                } else {
                    self.named_target(callee, value)
                }
            }
            Some(value) if !chained && value.kind() == "self" => Target {
                callee,
                receiver: Some(self.enclosing_type_name().unwrap_or_default()),
                names_own_type: true,
                declared: None,
                calls_a_method: false,
            },
            _ => Target::new(callee, Some(String::new())),
        };
        Called {
            target: target.of_a_method(),
            written: None,
            form: None,
        }
    }

    // constraint: a scoped call takes the last segment of its path as the receiver, `Self` as the type around it, and
    // a path keyword none; a qualified path, `<T as Trait>::f`, has no segment and records no receiver, and a path
    // from the root, `::a::f`, names no item of the file's blocks
    fn path_called(&self, callee: String, path: &TokenPath<'_>) -> Called {
        let segments = &path.segments;
        let Some(&last) = segments.last() else {
            return Called {
                target: Target::new(callee, None),
                written: None,
                form: None,
            };
        };
        let last_text = text(last, self.source);
        let (receiver, names_own_type) = match last.kind() {
            _ if last_text == "Self" => (self.enclosing_type_name(), true),
            "identifier" | "primitive_type" => (Some(last_text), false),
            _ => (None, false),
        };
        let form = segments
            .iter()
            .map(|segment| match segment.kind() {
                "identifier" | "primitive_type" => {
                    Some(PathSegment::Name(text(*segment, self.source)))
                }
                "self" => Some(PathSegment::SelfModule),
                "super" => Some(PathSegment::SuperModule),
                _ => None,
            })
            .collect::<Option<Vec<_>>>()
            .filter(|_| !path.rooted)
            .map(CallForm::Path);
        Called {
            target: Target {
                callee,
                receiver,
                names_own_type,
                declared: None,
                calls_a_method: false,
            },
            written: Some(self.spelled(path)),
            form,
        }
    }

    // constraint: a path from the root starts with `::`, as the syntax tree's paths spell it
    fn spelled(&self, path: &TokenPath<'_>) -> String {
        let mut spelled = String::new();
        for segment in &path.segments {
            if path.rooted || !spelled.is_empty() {
                spelled.push_str("::");
            }
            spelled.push_str(&String::from_utf8_lossy(&self.source[segment.byte_range()]));
        }
        if path.rooted && spelled.is_empty() {
            spelled.push_str("::");
        }
        spelled
    }

    // constraint: a closure's parameters run from the `|` that opens it to the next `|`, and each binds the names of
    // its pattern, before its `:`; its type binds nothing
    fn bind_closure(&self, tokens: &[Node<'_>], at: usize, reading: &mut Reading<'a, '_>) -> usize {
        let end = tokens[at..]
            .iter()
            .position(|token| token.kind() == "|")
            .map_or(tokens.len(), |end| at + end);
        let source = self.source;
        for parameter in tokens[at..end].split(|token| token.kind() == ",") {
            let pattern = parameter
                .iter()
                .copied()
                .take_while(|token| token.kind() != ":");
            bind_names(pattern, &mut |name| {
                reading.bound.push(&source[name.byte_range()]);
            });
        }
        end + 1
    }
}

// constraint: a name of a pattern binds, in a nested pattern too, unless a tree or a `::` follows it, which makes it a
// constructor or a path, `Some(x)` or `Kind::A { x }`, or a `:`, which makes it a field's label, `Point { x: at }`
fn bind_names<'t>(pattern: impl Iterator<Item = Node<'t>>, bind: &mut impl FnMut(Node<'t>)) {
    let mut pattern = pattern.peekable();
    while let Some(token) = pattern.next() {
        let constructor = pattern
            .peek()
            .is_some_and(|next| matches!(next.kind(), "token_tree" | "::" | ":"));
        match token.kind() {
            "identifier" if !constructor => bind(token),
            "token_tree" => bind_names(child_nodes(token), bind),
            _ => {}
        }
    }
}

// constraint: an `if` is a match arm's guard when a `=>` ends what follows it before a `,` or a `;` does, and an `if
// let` is a condition
fn is_guard(after: &[Node<'_>]) -> bool {
    after.first().is_some_and(|next| next.kind() != "let")
        && after
            .iter()
            .map(Node::kind)
            .find(|kind| matches!(*kind, "=>" | "," | ";"))
            == Some("=>")
}

fn delimiter(tree: Node<'_>) -> Option<&str> {
    tree.child(0).map(|open| open.kind())
}

// constraint: `default` and `union` are words the grammar keeps as keywords, and a method or a function can take
// either name, as `Options::default()` does
fn is_word(token: Node<'_>) -> bool {
    matches!(token.kind(), "identifier" | "default" | "union")
}

// constraint: a token tree keeps some keywords as identifiers, `in` and `move` among them, and no keyword names a
// callable, so `for x in (items)` calls nothing; a raw identifier, `r#in`, names one
fn is_callee(token: Node<'_>, source: &[u8]) -> bool {
    is_word(token)
        && !matches!(
            &source[token.byte_range()],
            b"abstract"
                | b"become"
                | b"box"
                | b"do"
                | b"dyn"
                | b"else"
                | b"final"
                | b"in"
                | b"macro"
                | b"move"
                | b"override"
                | b"priv"
                | b"ref"
                | b"return"
                | b"try"
                | b"typeof"
                | b"unsized"
                | b"virtual"
                | b"yield"
        )
}

// constraint: `rooted` marks a path written from the crate root, `::a::b`
struct TokenPath<'t> {
    segments: Vec<Node<'t>>,
    rooted: bool,
}

// constraint: a `|` opens a closure where an expression starts, after a delimiter, a separator, an assignment, an arm
// or `move`; after an operand it is the bitwise or. A token tree keeps `move` and `return` as identifiers
fn opens_closure(before: &[Node<'_>], source: &[u8]) -> bool {
    before.last().is_none_or(|previous| match previous.kind() {
        "(" | "[" | "{" | "," | "=" | ";" | "=>" | ":" => true,
        "identifier" => matches!(&source[previous.byte_range()], b"move" | b"return"),
        _ => false,
    })
}

// constraint: the name a `(` tree calls, and the tokens before it; a turbofish, `collect::<Vec<_>>(..)`, sits between
// the two
fn called_name<'t, 'n>(
    before: &'n [Node<'t>],
    source: &[u8],
) -> Option<(Node<'t>, &'n [Node<'t>])> {
    let (&last, rest) = before.split_last()?;
    let (name, rest) = if matches!(last.kind(), ">" | ">>") {
        let open = generics_start(rest, last.kind())?;
        let (colons, rest) = rest[..open].split_last()?;
        if colons.kind() != "::" {
            return None;
        }
        let (&name, rest) = rest.split_last()?;
        (name, rest)
    } else {
        (last, rest)
    };
    is_callee(name, source).then_some((name, rest))
}

// constraint: the `<` that opens the generic list `close` ends, `>>` closing two; the scan stops at a delimiter, a
// statement's end or an arm, which no generic list holds
fn generics_start(tokens: &[Node<'_>], close: &str) -> Option<usize> {
    let mut depth: usize = if close == ">>" { 2 } else { 1 };
    for at in (0..tokens.len()).rev() {
        match tokens[at].kind() {
            ">" => depth += 1,
            ">>" => depth += 2,
            "<" => depth -= 1,
            "<<" => depth = depth.checked_sub(2)?,
            "(" | "[" | "{" | ";" | "=" | "=>" => return None,
            _ => continue,
        }
        if depth == 0 {
            return Some(at);
        }
    }
    None
}

// constraint: where a `::` before `open` starts a turbofish, `parse::<Point>()`, the index past the `>` that closes
// it, `>>` closing two, since the types it names read no value; the scan stops at a statement's end or an arm, and a
// turbofish inside it, `::<Vec::<u8>, Point>`, ends within it, so the reading starts no scan there
fn generics_end(tokens: &[Node<'_>], open: usize) -> Option<usize> {
    if tokens.get(open)?.kind() != "<" {
        return None;
    }
    let mut depth: usize = 0;
    for (at, token) in tokens.iter().enumerate().skip(open) {
        depth = match token.kind() {
            "<" => depth + 1,
            ">" => depth.saturating_sub(1),
            ">>" => depth.saturating_sub(2),
            ";" | "=" | "=>" => return Some(at),
            _ => continue,
        };
        if depth == 0 {
            return Some(at + 1);
        }
    }
    Some(tokens.len())
}

// constraint: a cast's type, `value as *const Point` or `value as &'static dyn Kind`, runs from `from`, past the `as`,
// over pointer and reference marks, lifetimes, a path, its generic list and an array or slice type, and reads no value;
// a `<` after it opens generics, as Rust parses one, so a comparison of a cast is written in a tree, `(x as u8) < N`
fn cast_end(tokens: &[Node<'_>], from: usize) -> usize {
    let mut depth: usize = 0;
    for (at, token) in tokens.iter().enumerate().skip(from) {
        match token.kind() {
            "*" | "&" | "&&" | "'" | "const" | "static" | "mutable_specifier" | "identifier"
            | "primitive_type" | "::" | "self" | "super" | "crate" => {}
            "<" => depth += 1,
            ">" if depth > 0 => depth -= 1,
            ">>" if depth > 1 => depth -= 2,
            "," if depth > 0 => {}
            "token_tree" if depth > 0 || delimiter(*token) == Some("[") => {}
            _ => return at,
        }
    }
    tokens.len()
}

// constraint: Rust parses no struct expression where a condition or a `for`'s iterated value ends in a block, so a word
// before a `{` tree there is read, `if READY { .. }`; the scan goes back to the start of the expression
fn in_condition(before: &[Node<'_>], source: &[u8]) -> bool {
    for token in before.iter().rev() {
        match token.kind() {
            "if" | "while" | "match" => return true,
            "identifier" if &source[token.byte_range()] == b"in" => return true,
            ";" | "," | "=>" => return false,
            _ => {}
        }
    }
    false
}

// constraint: the segments of the path that ends at the `::` closing `tokens`, `a::b::` or `Vec::<u8>::`, generic
// arguments skipped; a path keyword is a segment, and the scan stops at anything else. A `::` with nothing of a path
// before it starts the path at the root, `::a::f`
fn path_before<'t>(tokens: &[Node<'t>]) -> TokenPath<'t> {
    let mut path = TokenPath {
        segments: Vec::new(),
        rooted: false,
    };
    let mut end = tokens.len();
    while end > 0 && tokens[end - 1].kind() == "::" {
        let mut before = end - 1;
        if let Some(close) = before.checked_sub(1).map(|close| tokens[close].kind())
            && matches!(close, ">" | ">>")
        {
            let Some(open) = generics_start(&tokens[..before - 1], close) else {
                break;
            };
            before = open;
            if before > 0 && tokens[before - 1].kind() == "::" {
                before -= 1;
            }
        }
        let segment = before
            .checked_sub(1)
            .map(|at| tokens[at])
            .filter(|segment| {
                matches!(
                    segment.kind(),
                    "identifier" | "self" | "super" | "crate" | "primitive_type"
                )
            });
        let Some(segment) = segment else {
            path.rooted = before == end - 1;
            break;
        };
        path.segments.push(segment);
        end = before - 1;
    }
    path.segments.reverse();
    path
}
