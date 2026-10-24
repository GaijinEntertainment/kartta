use kartta_lang::tree::{child_nodes, text};
use kartta_lang::{Symbol, SymbolId, SymbolKind};
use tree_sitter::Node;

use super::Emitter;
use super::syntax::{is_own_module, symbol};

// constraint: the arms of a `macro_rules!` of the file one of whose transcribers names an item by a metavariable, kept
// whole so an invocation takes the first arm its tokens match, as the expansion does, and the end of the block or
// module the definition is in scope to
pub(super) struct ItemMacro<'a> {
    arms: Vec<Arm<'a>>,
    scope_end: usize,
}

struct Arm<'a> {
    pattern: Vec<Matcher<'a>>,
    made: Vec<Made<'a>>,
}

enum Matcher<'a> {
    Token(&'a [u8]),
    Tree(&'static str, Vec<Self>),
    Bind(&'a [u8], Fragment),
    Repeat(Repetition<'a>),
}

impl<'a> Matcher<'a> {
    const fn stop(&self) -> Option<Stop<'a>> {
        match self {
            Self::Token(token) => Some(Stop::Token(token)),
            Self::Tree(open, _) => Some(Stop::Tree(open)),
            Self::Bind(..) | Self::Repeat(_) => None,
        }
    }
}

struct Repetition<'a> {
    inner: Vec<Matcher<'a>>,
    separator: Option<&'a [u8]>,
    operator: &'a [u8],
}

#[derive(Clone, Copy)]
enum Fragment {
    Ident,
    Lifetime,
    Literal,
    Tree,
    Block,
    Visibility,
    Type,
    Item,
    Phrase,
}

impl Fragment {
    const fn of(specifier: &[u8]) -> Self {
        match specifier {
            b"ident" => Self::Ident,
            b"lifetime" => Self::Lifetime,
            b"literal" => Self::Literal,
            b"tt" => Self::Tree,
            b"block" => Self::Block,
            b"vis" => Self::Visibility,
            b"ty" | b"path" => Self::Type,
            b"item" => Self::Item,
            _ => Self::Phrase,
        }
    }
}

// constraint: what a transcriber makes of the metavariable `name`: an item, or for `impl Trait for $name` the trait's
// tokens, each metavariable among them spelled as written
enum Made<'a> {
    Item {
        kind: SymbolKind,
        name: &'a [u8],
        visibility: Visibility<'a>,
    },
    Parent {
        name: &'a [u8],
        trait_path: Vec<&'a [u8]>,
    },
    Member {
        name: &'a [u8],
        kind: SymbolKind,
        member: &'a [u8],
        visibility: Visibility<'a>,
    },
}

impl<'a> Made<'a> {
    const fn name(&self) -> &'a [u8] {
        match self {
            Self::Item { name, .. } | Self::Parent { name, .. } | Self::Member { name, .. } => name,
        }
    }
}

#[derive(Clone, Copy)]
enum Visibility<'a> {
    Private,
    Public,
    Bound(&'a [u8]),
}

// constraint: what a fragment stops before where the pattern names it next, a token or a delimited tree; a fragment
// of an expression, a pattern or a statement also stops before a `,`, a `;` or a `=>` outside a tree, and a type or a
// path before one outside its `<` `>` too, since neither can hold one there
#[derive(Clone, Copy)]
enum Stop<'a> {
    Token(&'a [u8]),
    Tree(&'static str),
}

// constraint: a value names an item where it is one identifier, or a type of one identifier with its generic
// arguments, `Candidate<'_>`, which names the type its base does; a value bound inside a repetition belongs to the
// one pass of it that bound it, numbered from 1, and one outside every repetition to pass 0
struct Bound<'a, 't> {
    name: &'a [u8],
    first: Node<'t>,
    end: usize,
    names: bool,
    pass: usize,
}

const ITEM_KEYWORDS: [(&[u8], SymbolKind); 9] = [
    (b"struct", SymbolKind::Struct),
    (b"union", SymbolKind::Struct),
    (b"enum", SymbolKind::Enum),
    (b"fn", SymbolKind::Function),
    (b"const", SymbolKind::Constant),
    (b"static", SymbolKind::Variable),
    (b"trait", SymbolKind::Interface),
    (b"type", SymbolKind::TypeAlias),
    (b"mod", SymbolKind::Namespace),
];

// constraint: an outer doc comment is the attribute `#[doc = ".."]` to a macro, so it stays among the tokens
fn is_comment(token: Node<'_>) -> bool {
    matches!(token.kind(), "line_comment" | "block_comment") && !is_doc_comment(token)
}

fn is_doc_comment(token: Node<'_>) -> bool {
    token.child_by_field_name("outer").is_some()
}

fn open_delimiter(tree: Node<'_>) -> &'static str {
    match tree.child(0).map(|open| open.kind()) {
        Some("(") => "(",
        Some("[") => "[",
        Some("{") => "{",
        _ => "",
    }
}

fn is_tree(token: Node<'_>, open: &str) -> bool {
    token.kind() == "token_tree" && open_delimiter(token) == open
}

fn uncommented(node: Node<'_>) -> Vec<Node<'_>> {
    child_nodes(node)
        .filter(|token| !is_comment(*token))
        .collect()
}

fn inner_tokens(tree: Node<'_>) -> Vec<Node<'_>> {
    let mut tokens = uncommented(tree);
    tokens.pop();
    if !tokens.is_empty() {
        tokens.remove(0);
    }
    tokens
}

// constraint: an item ends at its `;` or its body; a `const`, a `static` or a `type` only at its `;`, since its value
// may be a struct expression's `{ .. }`
fn item_end(tokens: &[Node<'_>], from: usize, at_semicolon_only: bool) -> usize {
    tokens[from..]
        .iter()
        .position(|token| token.kind() == ";" || (!at_semicolon_only && is_tree(*token, "{")))
        .map_or(tokens.len(), |offset| from + offset + 1)
}

fn angle_depth(depth: usize, kind: &str) -> usize {
    match kind {
        "<" => depth + 1,
        ">" => depth.saturating_sub(1),
        ">>" => depth.saturating_sub(2),
        _ => depth,
    }
}

fn generics_end(header: &[Node<'_>]) -> usize {
    let mut depth = 0usize;
    for (at, token) in header.iter().enumerate() {
        depth = angle_depth(depth, token.kind());
        if depth == 0 {
            return at + 1;
        }
    }
    header.len()
}

// constraint: an `ident` fragment takes a keyword as it takes a name, `self` or `struct`, but not `_`
fn is_word(token: &[u8]) -> bool {
    token != b"_"
        && token
            .first()
            .is_some_and(|first| first.is_ascii_alphabetic() || *first == b'_')
        && token
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
}

fn takes_literal(token: Node<'_>) -> bool {
    token.kind().ends_with("_literal")
}

impl<'a> Emitter<'a> {
    fn bytes(&self, node: Node<'_>) -> &'a [u8] {
        &self.source[node.byte_range()]
    }

    // constraint: a later `macro_rules!` of the name shadows an earlier one by text order to the end of its own block
    // or module, so one that names no item is kept as a macro of no arm where it shadows an item macro
    pub(super) fn record_item_macro(&mut self, definition: Node<'_>) {
        let Some(name) = definition.child_by_field_name("name") else {
            return;
        };
        let name = self.bytes(name);
        let rules: Vec<(Node<'_>, Vec<Made<'a>>)> = child_nodes(definition)
            .filter(|rule| rule.kind() == "macro_rule")
            .filter_map(|rule| {
                let pattern = rule.child_by_field_name("left")?;
                let mut made = Vec::new();
                self.made(rule.child_by_field_name("right")?, &mut made);
                Some((pattern, made))
            })
            .collect();
        let names_items = rules.iter().any(|(_, made)| !made.is_empty());
        if !names_items && !self.item_macros.contains_key(name) {
            return;
        }
        let scope_end = definition
            .parent()
            .map_or(usize::MAX, |scope| scope.end_byte());
        let arms = if names_items {
            rules
                .into_iter()
                .map(|(pattern, made)| Arm {
                    pattern: self.matchers(&inner_tokens(pattern)),
                    made,
                })
                .collect()
        } else {
            Vec::new()
        };
        self.item_macros
            .entry(name)
            .or_default()
            .push(ItemMacro { arms, scope_end });
    }

    fn matchers(&self, tokens: &[Node<'_>]) -> Vec<Matcher<'a>> {
        let field = |token: Node<'_>, name: &str| {
            token
                .child_by_field_name(name)
                .map_or(&[][..], |field| self.bytes(field))
        };
        tokens
            .iter()
            .map(|&token| match token.kind() {
                "token_tree_pattern" => {
                    Matcher::Tree(open_delimiter(token), self.matchers(&inner_tokens(token)))
                }
                "token_binding_pattern" => {
                    Matcher::Bind(field(token, "name"), Fragment::of(field(token, "type")))
                }
                "token_repetition_pattern" => Matcher::Repeat(self.repetition(token)),
                _ => Matcher::Token(self.bytes(token)),
            })
            .collect()
    }

    // constraint: a repetition is `$ ( inner ) separator? operator`, its parts flat among the pattern's children but
    // for the separator, which the grammar keeps as no node, so it is the text between the `)` and the operator
    fn repetition(&self, pattern: Node<'_>) -> Repetition<'a> {
        let parts = uncommented(pattern);
        let operator = parts.last().map_or(&[][..], |last| self.bytes(*last));
        let close = parts
            .iter()
            .rposition(|part| part.kind() == ")")
            .unwrap_or_default();
        let separator = parts
            .last()
            .and_then(|last| self.source.get(parts[close].end_byte()..last.start_byte()))
            .map(<[u8]>::trim_ascii)
            .filter(|separator| !separator.is_empty());
        Repetition {
            inner: self.matchers(parts.get(2..close).unwrap_or_default()),
            separator,
            operator,
        }
    }

    // constraint: an item's own body holds its members and its code, never an item of the invocation's scope, so the
    // reading skips it and descends only into the trees around the items, a repetition's or a nested invocation's
    fn made(&self, transcriber: Node<'_>, made: &mut Vec<Made<'a>>) {
        let tokens = uncommented(transcriber);
        let mut at = 0;
        while let Some(&token) = tokens.get(at) {
            at += 1;
            if matches!(token.kind(), "token_tree" | "token_repetition") {
                self.made(token, made);
                continue;
            }
            let word = self.bytes(token);
            if word == b"impl" {
                at = self.made_impl(&tokens, at, made);
                continue;
            }
            let Some(&(keyword, kind)) = ITEM_KEYWORDS.iter().find(|(keyword, _)| *keyword == word)
            else {
                continue;
            };
            let Some(name) = tokens.get(at).filter(|name| name.kind() == "metavariable") else {
                continue;
            };
            made.push(Made::Item {
                kind,
                name: self.bytes(*name),
                visibility: self.visibility_before(&tokens, at - 1),
            });
            at = item_end(
                &tokens,
                at,
                matches!(keyword, b"const" | b"static" | b"type"),
            );
        }
    }

    // constraint: `impl<..> Trait for $name { .. }` adds the trait to the parents of the type the invocation binds, and
    // the functions, constants and types of an impl for `$name`, with or without a trait, are its members; an impl for
    // a type the transcriber spells adds nothing, and the reading resumes after the body
    fn made_impl(&self, tokens: &[Node<'_>], from: usize, made: &mut Vec<Made<'a>>) -> usize {
        let body = tokens[from..]
            .iter()
            .position(|token| is_tree(*token, "{"))
            .map_or(tokens.len(), |offset| from + offset);
        let header = &tokens[from..body];
        let generics = if header.first().is_some_and(|token| token.kind() == "<") {
            generics_end(header)
        } else {
            0
        };
        let target = header
            .iter()
            .rposition(|token| token.kind() == "for")
            .filter(|target| *target > generics);
        let Some(&name) = header
            .get(target.map_or(generics, |target| target + 1)..)
            .and_then(|target| match target {
                [name] if name.kind() == "metavariable" => Some(name),
                _ => None,
            })
        else {
            return body + 1;
        };
        let name = self.bytes(name);
        if let Some(target) = target {
            made.push(Made::Parent {
                name,
                trait_path: header[generics..target]
                    .iter()
                    .map(|token| self.bytes(*token))
                    .collect(),
            });
        }
        if let Some(&members) = tokens.get(body) {
            self.made_members(name, &inner_tokens(members), made);
        }
        body + 1
    }

    fn made_members(&self, owner: &'a [u8], tokens: &[Node<'_>], made: &mut Vec<Made<'a>>) {
        let mut at = 0;
        while let Some(&token) = tokens.get(at) {
            at += 1;
            if token.kind() == "token_repetition" {
                self.made_members(owner, &uncommented(token), made);
                continue;
            }
            let kind = match self.bytes(token) {
                b"fn" => SymbolKind::Method,
                b"const" => SymbolKind::Constant,
                b"type" => SymbolKind::TypeAlias,
                _ => continue,
            };
            let Some(&member) = tokens
                .get(at)
                .filter(|member| matches!(member.kind(), "identifier" | "metavariable"))
            else {
                continue;
            };
            made.push(Made::Member {
                name: owner,
                kind,
                member: self.bytes(member),
                visibility: self.visibility_before(tokens, at - 1),
            });
            at = item_end(tokens, at, kind != SymbolKind::Method);
        }
    }

    // constraint: the qualifiers a function takes before `fn`, `const`, `async`, `unsafe` and an `extern` ABI, stand
    // between the visibility and the keyword
    fn visibility_before(&self, tokens: &[Node<'_>], keyword: usize) -> Visibility<'a> {
        let qualifier = |token: &Node<'_>| {
            matches!(
                token.kind(),
                "const" | "async" | "unsafe" | "extern" | "string_literal" | "default"
            )
        };
        let before = &tokens[..keyword];
        let end = before
            .iter()
            .rposition(|token| !qualifier(token))
            .map_or(0, |last| last + 1);
        let mut before = before[..end].iter().rev();
        match before.next() {
            Some(token) if token.kind() == "metavariable" => Visibility::Bound(self.bytes(*token)),
            Some(token) if token.kind() == "pub" => Visibility::Public,
            Some(&scope)
                if is_tree(scope, "(")
                    && before.next().is_some_and(|token| token.kind() == "pub")
                    && !is_own_module(&text(scope, self.source)) =>
            {
                Visibility::Public
            }
            _ => Visibility::Private,
        }
    }

    // constraint: an invocation of a `macro_rules!` the file defined before it, called by its bare name, defines what
    // the first arm its tokens match makes, once for each identifier the arm's metavariable binds, as the expansion
    // would; an invocation no arm matches defines nothing the walk can name
    pub(super) fn record_macro_items(&mut self, invocation: Node<'_>) {
        if self.item_macros.is_empty() {
            return;
        }
        let statement = invocation
            .parent()
            .filter(|parent| parent.kind() == "expression_statement")
            .unwrap_or(invocation);
        for (made, name) in self.expand(invocation) {
            match made {
                MadeOf::Item(kind, exported) => {
                    self.push_made_item((kind, exported), name, statement);
                }
                MadeOf::Parent(trait_path) => {
                    let (target, impl_owner) = self.impl_target(text(name, self.source));
                    self.queue_parent(&target, trait_path, impl_owner);
                }
                MadeOf::Member(kind, exported, member) => {
                    self.push_made_member((kind, exported), name, member);
                }
            }
        }
    }

    // constraint: a member is owned by the type its impl names, as a written impl's is, on the line of the identifier
    // the invocation binds to the type
    fn push_made_member(
        &mut self,
        (kind, exported): (SymbolKind, bool),
        owner: Node<'_>,
        name: String,
    ) {
        let (target, impl_owner) = self.impl_target(text(owner, self.source));
        let qualified_name = format!("{target}::{name}");
        let mut member = symbol(name, qualified_name, kind, owner);
        member.owner = Some(target);
        member.exported = exported;
        let id = self.push_made(member);
        if let Some(at) = impl_owner {
            self.impl_owners[at].members.push(id);
        }
    }

    fn expand<'t>(&self, invocation: Node<'t>) -> Vec<(MadeOf, Node<'t>)> {
        let item_macro = invocation
            .child_by_field_name("macro")
            .filter(|name| name.kind() == "identifier")
            .and_then(|name| self.item_macros.get(self.bytes(name)))
            .and_then(|definitions| {
                definitions
                    .iter()
                    .rev()
                    .find(|definition| definition.scope_end > invocation.start_byte())
            });
        let arguments = child_nodes(invocation).find(|child| child.kind() == "token_tree");
        let (Some(item_macro), Some(arguments)) = (item_macro, arguments) else {
            return Vec::new();
        };
        let tokens = inner_tokens(arguments);
        let mut matching = Matching {
            source: self.source,
            bound: Vec::new(),
            pass: 0,
            passes: 0,
        };
        let Some(arm) = item_macro.arms.iter().find(|arm| {
            matching.bound.clear();
            matching.sequence(&arm.pattern, &tokens, (0, None)) == Some(tokens.len())
        }) else {
            return Vec::new();
        };
        arm.made
            .iter()
            .flat_map(|made| matching.made_of(made))
            .collect()
    }

    // constraint: an item an invocation in a function body makes is an item its block declares, as a written one is
    fn push_made_item(
        &mut self,
        (kind, exported): (SymbolKind, bool),
        name_token: Node<'_>,
        statement: Node<'_>,
    ) {
        let name = text(name_token, self.source);
        let (owner, qualified_name) = self.qualify(&name);
        let kind = if kind == SymbolKind::Function && owner.is_some() {
            SymbolKind::Method
        } else {
            kind
        };
        let mut item = symbol(name, qualified_name, kind, name_token);
        item.owner = owner;
        item.exported = exported || self.exported_by_scope();
        let id = self.push_made(item);
        self.record_block_item(statement, id);
    }

    fn push_made(&mut self, made: Symbol) -> SymbolId {
        let id = self.push_symbol(made);
        let flags = &mut self.raw.symbols[id.0 as usize].flags;
        *flags = flags.with_made(true);
        id
    }
}

#[derive(Clone)]
enum MadeOf {
    Item(SymbolKind, bool),
    Parent(String),
    Member(SymbolKind, bool, String),
}

// constraint: the matching takes each fragment greedily and a repetition as often as its tokens allow, with no
// backtracking, which the follow-set rules of `macro_rules!` make enough for a pattern the compiler accepts
struct Matching<'a, 't> {
    source: &'a [u8],
    bound: Vec<Bound<'a, 't>>,
    pass: usize,
    passes: usize,
}

impl<'a, 't> Matching<'a, 't> {
    fn bytes(&self, node: Node<'_>) -> &'a [u8] {
        &self.source[node.byte_range()]
    }

    // constraint: an outer doc comment of the invocation takes the place of a pattern's `#` and its `[..]` tree
    fn sequence(
        &mut self,
        pattern: &[Matcher<'a>],
        tokens: &[Node<'t>],
        (mut at, follow): (usize, Option<Stop<'a>>),
    ) -> Option<usize> {
        let mut index = 0;
        while let Some(matcher) = pattern.get(index) {
            let attribute = matches!(matcher, Matcher::Token(b"#"))
                && matches!(
                    pattern.get(index + 1),
                    Some(Matcher::Tree("[", _) | Matcher::Bind(_, Fragment::Tree))
                )
                && tokens.get(at).is_some_and(|token| is_doc_comment(*token));
            if attribute {
                at += 1;
                index += 2;
                continue;
            }
            let stop = pattern.get(index + 1).map_or(follow, Matcher::stop);
            at = self.one(matcher, tokens, (at, stop))?;
            index += 1;
        }
        Some(at)
    }

    fn one(
        &mut self,
        matcher: &Matcher<'a>,
        tokens: &[Node<'t>],
        (at, stop): (usize, Option<Stop<'a>>),
    ) -> Option<usize> {
        match matcher {
            Matcher::Token(token) => (self.bytes(*tokens.get(at)?) == *token).then_some(at + 1),
            Matcher::Tree(open, inner) => {
                let tree = *tokens.get(at)?;
                let inside = is_tree(tree, open).then(|| inner_tokens(tree))?;
                let end = self.sequence(inner, &inside, (0, None))?;
                (end == inside.len()).then_some(at + 1)
            }
            Matcher::Bind(name, fragment) => {
                let end = self.fragment(*fragment, tokens, (at, stop))?;
                if end > at {
                    let generic = tokens.get(at + 1).is_some_and(|next| next.kind() == "<");
                    self.bound.push(Bound {
                        name,
                        first: tokens[at],
                        end: tokens[end - 1].end_byte(),
                        names: tokens[at].kind() == "identifier"
                            && (end == at + 1 || matches!(fragment, Fragment::Type) && generic),
                        pass: self.pass,
                    });
                }
                Some(end)
            }
            Matcher::Repeat(repetition) => self.repeat(repetition, tokens, (at, stop)),
        }
    }

    fn repeat(
        &mut self,
        repetition: &Repetition<'a>,
        tokens: &[Node<'t>],
        (mut at, follow): (usize, Option<Stop<'a>>),
    ) -> Option<usize> {
        let stop = repetition.separator.map(Stop::Token).or(follow);
        let mut count = 0;
        loop {
            let start = match (count, repetition.separator) {
                (0, _) | (_, None) => at,
                (_, Some(separator)) => match tokens.get(at) {
                    Some(token) if self.bytes(*token) == separator => at + 1,
                    _ => break,
                },
            };
            let mark = self.bound.len();
            let outer = self.pass;
            self.passes += 1;
            self.pass = self.passes;
            let matched = self.sequence(&repetition.inner, tokens, (start, stop));
            self.pass = outer;
            match matched {
                Some(end) if end > start => {
                    at = end;
                    count += 1;
                }
                _ => {
                    self.bound.truncate(mark);
                    break;
                }
            }
            if repetition.operator == b"?" {
                break;
            }
        }
        (count > 0 || repetition.operator != b"+").then_some(at)
    }

    fn fragment(
        &self,
        fragment: Fragment,
        tokens: &[Node<'_>],
        (at, stop): (usize, Option<Stop<'a>>),
    ) -> Option<usize> {
        let token = tokens.get(at).copied();
        let one = |takes: fn(Node<'_>) -> bool| token.filter(|token| takes(*token)).map(|_| at + 1);
        match fragment {
            Fragment::Ident => token
                .filter(|token| token.kind() == "identifier" || is_word(self.bytes(*token)))
                .map(|_| at + 1),
            Fragment::Lifetime => one(|token| token.kind() == "lifetime"),
            Fragment::Literal => match token.map(|token| token.kind()) {
                Some("-") => tokens
                    .get(at + 1)
                    .filter(|token| takes_literal(**token))
                    .map(|_| at + 2),
                _ => one(takes_literal),
            },
            Fragment::Tree => one(|_| true),
            Fragment::Block => one(|token| is_tree(token, "{")),
            Fragment::Visibility => Some(match token {
                Some(token) if token.kind() == "pub" => {
                    at + 1
                        + usize::from(tokens.get(at + 1).is_some_and(|scope| is_tree(*scope, "(")))
                }
                _ => at,
            }),
            Fragment::Type | Fragment::Item | Fragment::Phrase => {
                self.phrase(fragment, tokens, (at, stop))
            }
        }
    }

    fn phrase(
        &self,
        fragment: Fragment,
        tokens: &[Node<'_>],
        (at, stop): (usize, Option<Stop<'a>>),
    ) -> Option<usize> {
        let mut depth = 0usize;
        let mut end = at;
        while let Some(&token) = tokens.get(end) {
            let kind = token.kind();
            let halts = match stop {
                Some(Stop::Token(stop)) => self.bytes(token) == stop,
                Some(Stop::Tree(open)) => is_tree(token, open),
                None => false,
            };
            let separates = !matches!(fragment, Fragment::Item) && matches!(kind, "," | ";" | "=>");
            if depth == 0 && (halts || separates) {
                break;
            }
            if matches!(fragment, Fragment::Type) {
                depth = angle_depth(depth, kind);
            }
            end += 1;
        }
        (end > at).then_some(end)
    }

    fn names(&self, name: &[u8]) -> Vec<(Node<'t>, usize)> {
        self.bound
            .iter()
            .filter(|value| value.name == name && value.names)
            .map(|value| (value.first, value.pass))
            .collect()
    }

    // constraint: what the arm makes for each identifier its metavariable binds; a member named by a metavariable is
    // made once for each identifier that binds, under the one type an impl binds, or paired by repetition with the
    // types where each binds as often, and otherwise under each type with the first
    fn made_of(&self, made: &Made<'_>) -> Vec<(MadeOf, Node<'t>)> {
        let owners = self.names(made.name());
        if let Made::Member {
            kind,
            member,
            visibility,
            ..
        } = made
            && member.starts_with(b"$")
        {
            let members = self.names(member);
            let member = |at: usize, pass: usize| {
                let name = text(members[at].0, self.source);
                MadeOf::Member(*kind, self.exports(*visibility, pass), name)
            };
            return match (owners.as_slice(), members.len()) {
                (_, 0) => Vec::new(),
                ([(owner, pass)], _) => (0..members.len())
                    .map(|at| (member(at, *pass), *owner))
                    .collect(),
                (_, count) if count == owners.len() => owners
                    .iter()
                    .enumerate()
                    .map(|(at, (owner, pass))| (member(at, *pass), *owner))
                    .collect(),
                _ => owners
                    .iter()
                    .map(|(owner, pass)| (member(0, *pass), *owner))
                    .collect(),
            };
        }
        owners
            .into_iter()
            .map(|(owner, pass)| {
                let of = match made {
                    Made::Item {
                        kind, visibility, ..
                    } => MadeOf::Item(*kind, self.exports(*visibility, pass)),
                    Made::Parent { trait_path, .. } => {
                        MadeOf::Parent(self.trait_path(trait_path, pass))
                    }
                    Made::Member {
                        kind,
                        member,
                        visibility,
                        ..
                    } => MadeOf::Member(
                        *kind,
                        self.exports(*visibility, pass),
                        String::from_utf8_lossy(member).into_owned(),
                    ),
                };
                (of, owner)
            })
            .collect()
    }

    // constraint: a value pairs with the name bound in the same pass of a repetition; one bound outside every
    // repetition pairs with each name, and a name outside every repetition with the first value
    fn value_text(&self, name: &[u8], pass: usize) -> Option<&'a [u8]> {
        let named = || self.bound.iter().filter(|value| value.name == name);
        named()
            .find(|value| value.pass == pass)
            .or_else(|| named().find(|value| value.pass == 0))
            .or_else(|| named().find(|_| pass == 0))
            .map(|value| &self.source[value.first.start_byte()..value.end])
    }

    fn exports(&self, visibility: Visibility<'_>, pass: usize) -> bool {
        match visibility {
            Visibility::Private => false,
            Visibility::Public => true,
            Visibility::Bound(name) => self
                .value_text(name, pass)
                .and_then(|written| written.strip_prefix(b"pub"))
                .is_some_and(|scope| !is_own_module(&String::from_utf8_lossy(scope))),
        }
    }

    // constraint: `$crate` names the macro's own crate, and the generic arguments of the trait are dropped, as a
    // written impl's parent drops them
    fn trait_path(&self, tokens: &[&[u8]], pass: usize) -> String {
        let mut path = String::new();
        for &token in tokens {
            let spelled = match self.value_text(token, pass) {
                Some(written) => written,
                None if token == b"$crate" => b"crate",
                None => token,
            };
            path.push_str(&String::from_utf8_lossy(spelled));
        }
        path.split('<').next().unwrap_or_default().to_owned()
    }
}
