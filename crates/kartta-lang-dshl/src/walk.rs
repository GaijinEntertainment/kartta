use std::ops::Range;

use kartta_lang::tree::{self, signature, text};
use kartta_lang::{
    Access, CallRef, FILE_SCOPE, RawDirective, RawExtraction, Receiver, Symbol, SymbolId,
    SymbolKind, UseRef,
};
use tree_sitter::{Node, Tree, TreeCursor};

const INCLUDE_FORM: u32 = 0;

struct Scope {
    names: Range<u32>,
    owner: String,
    end: usize,
}

struct Walker<'s> {
    source: &'s [u8],
    scopes: Vec<Scope>,
    raw: RawExtraction,
}

pub(crate) fn raw_walk(tree: &Tree, source: &[u8]) -> RawExtraction {
    let mut walker = Walker {
        source,
        scopes: Vec::new(),
        raw: RawExtraction::default(),
    };
    let mut cursor = tree.walk();
    loop {
        let node = cursor.node();
        walker.close_scopes(node.start_byte());
        walker.visit(node, &cursor);
        if !cursor.goto_first_child() && !next_in_preorder(&mut cursor) {
            break;
        }
    }

    walker.raw
}

fn next_in_preorder(cursor: &mut TreeCursor<'_>) -> bool {
    loop {
        if cursor.goto_next_sibling() {
            return true;
        }
        if !cursor.goto_parent() {
            return false;
        }
    }
}

fn line(node: Node<'_>) -> u32 {
    tree::line(node.start_position().row)
}

fn end_line(node: Node<'_>) -> u32 {
    tree::line(node.end_position().row)
}

// constraint: `arguments: 0` reads as a call that counted nothing, which binds a macro of parameters, yet the shader
// compiler refuses that call, so a call and a macro each count one past their entries and compare as written
fn count_past_zero(list: Option<Node<'_>>, kind: &str) -> u8 {
    list.map_or(0, |list| {
        let mut cursor = list.walk();
        let entries = list.named_children(&mut cursor);
        u8::try_from(entries.filter(|entry| entry.kind() == kind).count() + 1).unwrap_or(u8::MAX)
    })
}

impl Walker<'_> {
    fn close_scopes(&mut self, at: usize) {
        while self.scopes.last().is_some_and(|scope| at >= scope.end) {
            self.scopes.pop();
        }
    }

    fn caller(&self) -> SymbolId {
        self.scopes
            .last()
            .map_or(FILE_SCOPE, |scope| SymbolId(scope.names.start))
    }

    fn visit(&mut self, node: Node<'_>, cursor: &TreeCursor<'_>) {
        match node.kind() {
            "shader_declaration" => self.shader(node),
            "block_declaration" => self.named_scope(node, SymbolKind::Interface),
            "macro_definition" => self.macro_definition(node),
            "render_pass_declaration"
            | "render_pass_group_declaration"
            | "framebuffer_layout_declaration" => self.named_scope(node, SymbolKind::Class),
            "variable_declaration" | "sampler_declaration" => self.variable(node),
            "preshader_variable" => self.member(node, SymbolKind::Field),
            "bool_declaration" => self.member(node, SymbolKind::Constant),
            "interval_declaration" => self.interval(node),
            "supports_statement" => self.supports(node),
            "macro_call" => self.macro_call(node),
            "include_directive" => self.include(node),
            "hlsl_code" => self.hlsl_includes(node),
            "identifier" => self.identifier(node, cursor),
            _ => {}
        }
    }

    fn push(&mut self, mut symbol: Symbol) -> SymbolId {
        symbol.exported = true;
        self.raw.push_symbol(symbol)
    }

    fn declared(&mut self, node: Node<'_>, name: Node<'_>, kind: SymbolKind) -> SymbolId {
        let name = text(name, self.source);
        let mut symbol = Symbol::new(String::new(), kind, line(node), end_line(node));
        symbol.qualified_name = self
            .scopes
            .last()
            .map_or_else(|| name.clone(), |scope| format!("{}.{name}", scope.owner));
        symbol.owner = self.scopes.last().map(|scope| scope.owner.clone());
        symbol.name = name;
        symbol.signature = signature(node, self.source);
        self.push(symbol)
    }

    fn open(&mut self, node: Node<'_>, names: Range<u32>, owner: String) {
        self.scopes.push(Scope {
            names,
            owner,
            end: node.end_byte(),
        });
    }

    fn shader(&mut self, node: Node<'_>) {
        let mut cursor = node.walk();
        let mut declared: Option<(Range<u32>, String)> = None;
        for name in node.children_by_field_name("name", &mut cursor) {
            let SymbolId(id) = self.declared(node, name, SymbolKind::Class);
            match &mut declared {
                Some((names, _)) => names.end = id + 1,
                None => declared = Some((id..id + 1, text(name, self.source))),
            }
        }
        if let Some((names, owner)) = declared {
            self.open(node, names, owner);
        }
    }

    fn named_scope(&mut self, node: Node<'_>, kind: SymbolKind) {
        if let Some(name) = node.child_by_field_name("name") {
            let SymbolId(id) = self.declared(node, name, kind);
            self.open(node, id..id + 1, text(name, self.source));
        }
    }

    // constraint: `define_macro_if_not_defined` defines the macro only where the build holds none of its name yet, so
    // a game that defines the macro first runs its own wherever the library calls it, and the shader compiler's lexer
    // takes any definition into one table per script, so a macro in a body stands free as one at file scope
    fn macro_definition(&mut self, node: Node<'_>) {
        let declared = self.raw.symbols.len();
        self.named_scope(node, SymbolKind::Macro);
        if let Some(symbol) = self.raw.symbols.get_mut(declared) {
            symbol.owner = None;
            symbol.qualified_name.clone_from(&symbol.name);
            let parameters = count_past_zero(node.child_by_field_name("parameters"), "identifier");
            symbol.fewest_arguments = parameters;
            symbol.most_arguments = parameters;
            if node.child(0).map(|keyword| keyword.kind()) == Some("define_macro_if_not_defined") {
                symbol.flags = symbol.flags.with_fallback(true);
            }
        }
    }

    fn variable(&mut self, node: Node<'_>) {
        let local = node
            .child_by_field_name("modifier")
            .is_some_and(|modifier| modifier.kind() == "local");
        if !local {
            let kind = if self.scopes.is_empty() {
                SymbolKind::Variable
            } else {
                SymbolKind::Field
            };
            self.member(node, kind);
        }
    }

    fn member(&mut self, node: Node<'_>, kind: SymbolKind) {
        if let Some(name) = node.child_by_field_name("name") {
            self.declared(node, name, kind);
        }
    }

    fn interval(&mut self, node: Node<'_>) {
        let Some(name) = node.child_by_field_name("name") else {
            return;
        };
        let SymbolId(interval) = self.declared(node, name, SymbolKind::Enum);
        let owner = text(name, self.source);
        self.open(node, interval..interval + 1, owner);
        let mut cursor = node.walk();
        for value in node.children_by_field_name("value", &mut cursor) {
            if let Some(name) = value.child_by_field_name("name") {
                self.declared(value, name, SymbolKind::EnumVariant);
            }
        }
        self.scopes.pop();
    }

    fn supports(&mut self, node: Node<'_>) {
        let declared = self.scopes.last().map_or(0..0, |scope| {
            scope.names.start as usize..scope.names.end as usize
        });
        let mut cursor = node.walk();
        let blocks = node.children_by_field_name("block", &mut cursor);
        for block in blocks.filter(|block| block.kind() == "identifier") {
            let name = text(block, self.source);
            let lacking = self.raw.symbols[declared.clone()]
                .iter_mut()
                .map(|symbol| &mut symbol.parents)
                .filter(|parents| !parents.contains(&name));
            for parents in lacking {
                parents.push(name.clone());
            }
            self.reference(name, block);
        }
    }

    fn macro_call(&mut self, node: Node<'_>) {
        if let Some(name) = node.child_by_field_name("macro") {
            self.raw.calls.push(CallRef {
                site_of: self.caller(),
                callee: text(name, self.source),
                receiver: None,
                line: line(node),
                invokes_a_macro: false,
                calls_a_method: false,
                arguments: count_past_zero(node.child_by_field_name("arguments"), "macro_argument"),
            });
        }
    }

    fn include(&mut self, node: Node<'_>) {
        if let Some(path) = node.child_by_field_name("path") {
            let spec = text(path, self.source).trim_matches('"').to_owned();
            self.raw
                .directives
                .push(RawDirective::new(INCLUDE_FORM, spec, line(node)));
        }
    }

    fn hlsl_includes(&mut self, node: Node<'_>) {
        let first = line(node);
        let code = &self.source[node.byte_range()];
        for (at, row) in code.split(|&byte| byte == b'\n').enumerate() {
            if let Some(spec) = hlsl_include(row) {
                let offset = u32::try_from(at).unwrap_or(u32::MAX);
                self.raw.directives.push(RawDirective::new(
                    INCLUDE_FORM,
                    spec,
                    first.saturating_add(offset),
                ));
            }
        }
    }

    fn identifier(&mut self, node: Node<'_>, cursor: &TreeCursor<'_>) {
        let Some(parent) = node.parent() else {
            return;
        };
        if names_a_use(parent.kind(), cursor.field_name()) {
            let name = text(node, self.source);
            self.reference(name, node);
        }
    }

    fn reference(&mut self, name: String, node: Node<'_>) {
        self.raw.uses.push(UseRef {
            site_of: self.caller(),
            name,
            receiver: Receiver::Bare,
            access: Access::Read,
            line: line(node),
        });
    }
}

// constraint: the parent and the field decide whether an identifier names a variable, an interval, a bool, a block or
// a shader, or something that is not a declaration of the shader script: a keyword-like value, a channel mask, an
// intrinsic, a hardware feature, a macro argument, or the range of an interval a comparison tests against
fn names_a_use(parent: &str, field: Option<&str>) -> bool {
    match parent {
        "binary_condition"
        | "binary_expression"
        | "parenthesized_condition"
        | "parenthesized_expression"
        | "initializer_list"
        | "macro_body" => true,
        "comparison" => field == Some("left"),
        "maybe_condition" => field == Some("variable"),
        "shader_condition" => field == Some("right"),
        "assume_statement" | "hlsl_declare_directive" => field == Some("interval"),
        "subpass_reference" => field == Some("pass"),
        "hlsl_bool_directive" => field == Some("name"),
        "not_condition" | "unary_expression" | "swizzle_expression" => field == Some("operand"),
        "if_statement" | "hlsl_if_directive" | "hlsl_elif_directive" => field == Some("condition"),
        "bool_declaration"
        | "preshader_variable"
        | "variable_declaration"
        | "variable_attribute" => field == Some("value"),
        "function_call" => field == Some("argument"),
        "vector_expression" => field == Some("component"),
        _ => false,
    }
}

fn hlsl_include(row: &[u8]) -> Option<String> {
    let rest = row.trim_ascii_start().strip_prefix(b"#")?;
    let rest = rest
        .trim_ascii_start()
        .strip_prefix(b"include")?
        .trim_ascii_start();
    let close = match rest.first()? {
        b'"' => b'"',
        b'<' => b'>',
        _ => return None,
    };
    let inner = &rest[1..];
    let spec = &inner[..inner.iter().position(|&byte| byte == close)?];
    (!spec.is_empty()).then(|| String::from_utf8_lossy(spec).into_owned())
}
