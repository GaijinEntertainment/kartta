mod data;
mod declarator;
mod identity;
mod name;

use std::collections::{HashMap, HashSet};

use kartta_lang::tree::{preorder, signature, text};
use kartta_lang::{
    CallRef, Callable, CallableRole, FoldedDeclaration, NO_CALLER_ID, RawDirective, RawExtraction,
    Symbol, SymbolFlags, SymbolId, SymbolKind,
};
use tree_sitter::{Node, Tree};

use crate::banner::is_generated_source;

use self::data::{
    alias_declarator_name, at_namespace_scope, data_declarator, data_signature, field_declarator,
    field_owner, has_qualifier, has_storage_class,
};
use self::declarator::{function_symbol, is_macro_invocation, is_pure_virtual};
use self::name::{
    call_target, class_symbol, head_scope, include_spec, is_simple_identifier, type_base_name,
};

pub(crate) fn raw_walk(tree: &Tree, source: &[u8]) -> RawExtraction {
    let mut replayer = Replayer::new(source);

    preorder(tree, |node| {
        if node.is_named() {
            replayer.handle(node);
        }
        true
    });

    replayer.flush_prototypes();
    replayer.raw
}

struct FunctionScope {
    end: usize,
    id: SymbolId,
}

struct ClassScope {
    end: usize,
    name: String,
}

struct NamespaceScope {
    end: usize,
    name: Option<String>,
}

struct Prototype {
    symbol: Symbol,
    key: Option<String>,
    in_class: bool,
}

struct Replayer<'s> {
    source: &'s [u8],
    functions: Vec<FunctionScope>,
    classes: Vec<ClassScope>,
    namespaces: Vec<NamespaceScope>,
    prototypes: Vec<Prototype>,
    callable_keys: HashSet<String>,
    out_of_line: HashMap<String, Vec<SymbolId>>,
    generated: bool,
    symbol_count: u32,
    raw: RawExtraction,
}

impl<'s> Replayer<'s> {
    fn new(source: &'s [u8]) -> Self {
        Self {
            source,
            functions: Vec::new(),
            classes: Vec::new(),
            namespaces: Vec::new(),
            prototypes: Vec::new(),
            callable_keys: HashSet::new(),
            out_of_line: HashMap::new(),
            generated: is_generated_source(source),
            symbol_count: 0,
            raw: RawExtraction::default(),
        }
    }

    fn handle(&mut self, node: Node<'_>) {
        self.close_scopes(node.start_byte());

        match node.kind() {
            "namespace_definition" => self.namespaces.push(NamespaceScope {
                end: node.end_byte(),
                name: node
                    .child_by_field_name("name")
                    .map(|name| text(name, self.source)),
            }),
            "function_definition" => self.define_function(node),
            "field_declaration" if is_pure_virtual(node) => self.declare_pure_virtual(node),
            "field_declaration" => {
                self.queue_prototype(node);
                self.push_fields(node);
            }
            "declaration" if self.functions.is_empty() => {
                self.queue_prototype(node);
                self.push_data(node);
            }
            "class_specifier" | "struct_specifier" | "union_specifier" => self.open_class(node),
            "enum_specifier" if node.child_by_field_name("body").is_some() => {
                self.define_enum(node);
            }
            "alias_declaration" => {
                if let Some(name) = node.child_by_field_name("name") {
                    self.push_type_alias(text(name, self.source), node);
                }
            }
            "type_definition" => {
                let mut cursor = node.walk();
                let names = node
                    .children_by_field_name("declarator", &mut cursor)
                    .filter_map(alias_declarator_name);
                for name in names {
                    self.push_type_alias(text(name, self.source), node);
                }
            }
            "call_expression" => self.record_call(node),
            "new_expression" => self.record_construction(node),
            "preproc_include" => {
                if let Some((spec, form)) = include_spec(node, self.source) {
                    let line = line(node.start_position().row);
                    self.raw
                        .directives
                        .push(RawDirective::new(form, spec, line));
                }
            }
            _ => {}
        }
    }

    fn close_scopes(&mut self, start: usize) {
        while self
            .functions
            .last()
            .is_some_and(|scope| scope.end <= start)
        {
            self.functions.pop();
        }
        while self.classes.last().is_some_and(|scope| scope.end <= start) {
            self.classes.pop();
        }
        while self
            .namespaces
            .last()
            .is_some_and(|scope| scope.end <= start)
        {
            self.namespaces.pop();
        }
    }

    fn define_function(&mut self, node: Node<'_>) {
        let Some(symbol) = self.callable_symbol(node) else {
            return;
        };
        let key = self.callable_key(node);
        let out_of_line = symbol.owner.is_some() && self.classes.is_empty();

        let id = self.push_symbol(symbol);
        if out_of_line && let Some(key) = &key {
            self.out_of_line.entry(key.clone()).or_default().push(id);
        }
        self.record_callable(id, key, CallableRole::Definition);
        self.functions.push(FunctionScope {
            end: node.end_byte(),
            id,
        });
    }

    fn declare_pure_virtual(&mut self, node: Node<'_>) {
        let Some(symbol) = self.callable_symbol(node) else {
            return;
        };
        let key = self.callable_key(node);

        let id = self.push_symbol(symbol);
        self.record_callable(id, key, CallableRole::Declaration);
    }

    fn queue_prototype(&mut self, node: Node<'_>) {
        let Some(mut symbol) = self.callable_symbol(node) else {
            return;
        };
        self.apply_namespace(&mut symbol);
        let key = self.callable_key(node);

        self.prototypes.push(Prototype {
            symbol,
            key,
            in_class: !self.classes.is_empty(),
        });
    }

    fn callable_symbol(&self, node: Node<'_>) -> Option<Symbol> {
        let mut symbol = function_symbol(node, self.source)?;
        self.attribute_to_class(&mut symbol);
        self.apply_linkage(&mut symbol, node);
        self.apply_flags(&mut symbol);

        (!is_macro_invocation(node, &symbol)).then_some(symbol)
    }

    fn open_class(&mut self, node: Node<'_>) {
        let Some(mut symbol) = class_symbol(node, self.source) else {
            return;
        };
        self.nest_in_class(&mut symbol, node);
        if symbol.owner.is_none()
            && let Some(scope) = head_scope(node, self.source)
        {
            symbol.qualified_name = format!("{}::{}", self.written_scope(&scope), symbol.name);
        }
        self.apply_flags(&mut symbol);

        let name = symbol.name.clone();
        self.push_symbol(symbol);
        self.classes.push(ClassScope {
            end: node.end_byte(),
            name,
        });
    }

    fn define_enum(&mut self, node: Node<'_>) {
        let Some(name) = node.child_by_field_name("name") else {
            return;
        };
        let mut symbol = exported_symbol(text(name, self.source), SymbolKind::Enum, node);
        symbol.flags = self.base_flags();
        self.nest_in_class(&mut symbol, node);
        self.push_symbol(symbol);
    }

    fn push_type_alias(&mut self, name: String, node: Node<'_>) {
        let mut symbol = exported_symbol(name, SymbolKind::TypeAlias, node);
        self.nest_in_class(&mut symbol, node);
        self.apply_linkage(&mut symbol, node);
        self.apply_flags(&mut symbol);
        self.push_symbol(symbol);
    }

    fn push_fields(&mut self, node: Node<'_>) {
        if node.child_by_field_name("type").is_none() || node.has_error() {
            return;
        }
        let mut cursor = node.walk();
        let mut names = node
            .children_by_field_name("declarator", &mut cursor)
            .filter_map(field_declarator)
            .peekable();
        if names.peek().is_none() {
            return;
        }
        let Some(owner) = field_owner(node, self.source) else {
            return;
        };
        let signature = signature(node, self.source);

        for name in names {
            let mut symbol = exported_symbol(text(name, self.source), SymbolKind::Field, node);
            symbol.qualified_name = format!("{owner}::{}", symbol.name);
            symbol.owner = Some(owner.clone());
            symbol.signature.clone_from(&signature);
            symbol.flags = self.base_flags();
            self.apply_linkage(&mut symbol, node);
            self.push_symbol(symbol);
        }
    }

    fn push_data(&mut self, node: Node<'_>) {
        if node.child_by_field_name("type").is_none()
            || node.has_error()
            || !at_namespace_scope(node)
        {
            return;
        }
        let constexpr = has_qualifier(node, "constexpr", self.source);
        let declared_const = has_qualifier(node, "const", self.source);
        let external = has_storage_class(node, "extern", self.source)
            || has_storage_class(node, "inline", self.source);
        let signature = data_signature(node, self.source);

        let mut cursor = node.walk();
        for declarator in node.children_by_field_name("declarator", &mut cursor) {
            let Some((name, const_pointer)) = data_declarator(declarator, self.source) else {
                continue;
            };
            let kind = if constexpr || const_pointer.unwrap_or(declared_const) {
                SymbolKind::Constant
            } else {
                SymbolKind::Variable
            };
            let mut symbol = exported_symbol(text(name, self.source), kind, node);
            symbol.exported = external || kind != SymbolKind::Constant;
            symbol.signature.clone_from(&signature);
            symbol.flags = self.base_flags();
            self.apply_linkage(&mut symbol, node);
            self.push_symbol(symbol);
        }
    }

    fn record_call(&mut self, node: Node<'_>) {
        if self.symbol_count == 0 {
            return;
        }
        let Some((callee, receiver)) = call_target(node, self.source) else {
            return;
        };

        self.raw.calls.push(CallRef {
            caller_id: self.caller(),
            callee,
            receiver,
            line: line(node.start_position().row),
        });
    }

    fn record_construction(&mut self, node: Node<'_>) {
        if self.symbol_count == 0 {
            return;
        }
        let Some(type_node) = node.child_by_field_name("type") else {
            return;
        };
        let callee = type_base_name(type_node, self.source);
        if !is_simple_identifier(&callee) {
            return;
        }

        self.raw.calls.push(CallRef {
            caller_id: self.caller(),
            callee,
            receiver: None,
            line: line(node.start_position().row),
        });
    }

    fn caller(&self) -> SymbolId {
        self.functions.last().map_or(NO_CALLER_ID, |scope| scope.id)
    }

    fn attribute_to_class(&self, symbol: &mut Symbol) {
        if symbol.owner.is_some() {
            return;
        }
        let Some(class) = self.classes.last() else {
            return;
        };

        symbol.kind = SymbolKind::Method;
        symbol.qualified_name = format!("{}::{}", class.name, symbol.name);
        symbol.owner = Some(class.name.clone());
    }

    fn nest_in_class(&self, symbol: &mut Symbol, node: Node<'_>) {
        let Some(class) = self.classes.last() else {
            return;
        };
        let in_body = std::iter::successors(node.parent(), Node::parent)
            .take_while(|ancestor| ancestor.end_byte() < class.end)
            .any(|ancestor| matches!(ancestor.kind(), "function_definition" | "lambda_expression"));
        if in_body {
            return;
        }

        symbol.qualified_name = format!("{}::{}", class.name, symbol.name);
        symbol.owner = Some(class.name.clone());
    }

    fn push_symbol(&mut self, mut symbol: Symbol) -> SymbolId {
        self.apply_namespace(&mut symbol);

        let id = SymbolId(self.symbol_count);
        self.symbol_count += 1;
        self.raw.symbols.push(symbol);
        id
    }

    fn apply_namespace(&self, symbol: &mut Symbol) {
        if symbol.owner.is_some() || symbol.qualified_name != symbol.name {
            return;
        }

        let prefix: Vec<&str> = self
            .namespaces
            .iter()
            .filter_map(|scope| scope.name.as_deref())
            .collect();
        if !prefix.is_empty() {
            symbol.qualified_name = format!("{}::{}", prefix.join("::"), symbol.name);
        }
    }

    fn written_scope(&self, written: &str) -> String {
        let enclosing: Vec<&str> = self
            .namespaces
            .iter()
            .filter_map(|scope| scope.name.as_deref())
            .flat_map(|name| name.split("::"))
            .collect();
        let first = written.split("::").next().unwrap_or(written);
        let kept = enclosing
            .iter()
            .rposition(|name| *name == first)
            .unwrap_or(enclosing.len());

        enclosing[..kept]
            .iter()
            .copied()
            .chain([written])
            .collect::<Vec<_>>()
            .join("::")
    }

    fn apply_linkage(&self, symbol: &mut Symbol, node: Node<'_>) {
        let in_anonymous_namespace = self.namespaces.iter().any(|scope| scope.name.is_none());
        if in_anonymous_namespace
            || (symbol.owner.is_none() && has_storage_class(node, "static", self.source))
        {
            symbol.exported = false;
        }
    }

    const fn base_flags(&self) -> SymbolFlags {
        SymbolFlags::empty().with_generated(self.generated)
    }

    fn apply_flags(&self, symbol: &mut Symbol) {
        let entry =
            (self.namespaces.is_empty() && is_entry_point(symbol)) || is_implicitly_called(symbol);
        symbol.flags = self.base_flags().with_entry(entry);
    }

    fn flush_prototypes(&mut self) {
        for Prototype {
            symbol,
            key,
            in_class,
        } in std::mem::take(&mut self.prototypes)
        {
            if !key
                .as_ref()
                .is_some_and(|key| self.callable_keys.contains(key))
            {
                let id = self.push_symbol(symbol);
                self.record_callable(id, key, CallableRole::Declaration);
                continue;
            }
            if in_class
                && let Some(definitions) = key.as_ref().and_then(|key| self.out_of_line.get(key))
            {
                let line = symbol.line;
                let folded = definitions
                    .iter()
                    .map(|&definition| FoldedDeclaration { definition, line });
                self.raw.folded_declarations.extend(folded);
            }
        }
    }

    fn record_callable(&mut self, symbol: SymbolId, key: Option<String>, role: CallableRole) {
        let Some(key) = key else {
            return;
        };
        self.callable_keys.insert(key.clone());
        if self.raw.symbols[symbol.0 as usize].exported {
            self.raw.callables.push(Callable { symbol, key, role });
        }
    }
}

fn exported_symbol(name: String, kind: SymbolKind, node: Node<'_>) -> Symbol {
    let mut symbol = Symbol::new(
        name,
        kind,
        line(node.start_position().row),
        line(node.end_position().row),
    );
    symbol.exported = true;
    symbol
}

fn is_entry_point(symbol: &Symbol) -> bool {
    symbol.owner.is_none()
        && symbol.kind == SymbolKind::Function
        && matches!(
            symbol.name.as_str(),
            "main" | "wmain" | "WinMain" | "wWinMain" | "DagorWinMain"
        )
}

// constraint: the language calls a constructor for each object it makes, of which only a `new` expression is recorded
// as a call, a destructor for each object it ends and an operator for an expression
fn is_implicitly_called(symbol: &Symbol) -> bool {
    let is_operator = symbol
        .name
        .strip_prefix("operator")
        .is_some_and(|rest| !rest.starts_with(|c: char| c.is_ascii_alphanumeric() || c == '_'));
    matches!(symbol.kind, SymbolKind::Function | SymbolKind::Method)
        && (is_operator
            || symbol.name.starts_with('~')
            || symbol.owner.as_deref() == Some(symbol.name.as_str()))
}

#[expect(
    clippy::cast_possible_truncation,
    reason = "tree-sitter counts rows in a uint32_t, which the binding only widens to usize"
)]
const fn line(row: usize) -> u32 {
    row as u32 + 1
}

#[cfg(test)]
mod tests {
    use kartta_lang::Language;

    use super::*;
    use crate::{Cpp, cpp_parser};

    const SAMPLE: &[u8] = br#"
#include "core/util.h"
#include <vector>

namespace ns {

class Base {
public:
    virtual int compute() = 0;
};

class Widget : public Base {
public:
    int compute() override {
        return helper(size_);
    }

    void render() {
        compute();
        painter_.draw(this);
    }

private:
    int size_;
    Painter painter_;
};

}

int ns::Widget::area() const {
    return compute();
}

int *Factory::make() {
    return new int(0);
}

static int helper(int v) {
    return v;
}

enum Color { Red, Green };

TEST(MySuite, DoesThing) {
    helper(1);
}
"#;

    const HEADERISH: &[u8] = br"
namespace {
void local_helper() {}
}

class Engine {
public:
    void start();
    void stop();
    void run() {
        auto w = new gfx::Window<int>();
        start();
    }
};

int free_proto(int x);

int defined_and_declared(int x);
int defined_and_declared(int x) { return x; }
";

    fn extract(source: &[u8]) -> RawExtraction {
        Cpp::new().extract(source)
    }

    fn names(raw: &RawExtraction) -> Vec<&str> {
        raw.symbols
            .iter()
            .map(|symbol| symbol.name.as_str())
            .collect()
    }

    fn qualified_names(raw: &RawExtraction) -> Vec<&str> {
        raw.symbols
            .iter()
            .map(|symbol| symbol.qualified_name.as_str())
            .collect()
    }

    fn callees(raw: &RawExtraction) -> Vec<&str> {
        raw.calls.iter().map(|call| call.callee.as_str()).collect()
    }

    fn symbol<'r>(raw: &'r RawExtraction, name: &str) -> &'r Symbol {
        raw.symbols
            .iter()
            .find(|symbol| symbol.name == name)
            .unwrap_or_else(|| panic!("no symbol {name} in {:?}", names(raw)))
    }

    fn qualified<'r>(raw: &'r RawExtraction, qualified_name: &str) -> &'r Symbol {
        raw.symbols
            .iter()
            .find(|symbol| symbol.qualified_name == qualified_name)
            .unwrap_or_else(|| panic!("no symbol {qualified_name} in {:?}", qualified_names(raw)))
    }

    fn id_of(raw: &RawExtraction, name: &str) -> SymbolId {
        let position = raw.symbols.iter().position(|symbol| symbol.name == name);
        SymbolId(
            u32::try_from(position.unwrap_or_else(|| panic!("no symbol {name}")))
                .expect("a small fixture"),
        )
    }

    #[test]
    fn extracts_qualified_names_and_kinds_in_preorder() {
        let raw = extract(SAMPLE);

        assert_eq!(
            qualified_names(&raw),
            [
                "ns::Base",
                "Base::compute",
                "ns::Widget",
                "Widget::compute",
                "Widget::render",
                "Widget::size_",
                "Widget::painter_",
                "Widget::area",
                "Factory::make",
                "helper",
                "Color",
            ]
        );
        assert_eq!(symbol(&raw, "Base").kind, SymbolKind::Class);
        assert_eq!(symbol(&raw, "Color").kind, SymbolKind::Enum);
    }

    #[test]
    fn inline_member_is_attributed_to_enclosing_class() {
        let raw = extract(SAMPLE);

        let render = symbol(&raw, "render");
        assert_eq!(render.kind, SymbolKind::Method);
        assert_eq!(render.owner.as_deref(), Some("Widget"));
        assert_eq!(render.qualified_name, "Widget::render");
    }

    #[test]
    fn out_of_line_method_takes_scope_qualifier_as_owner() {
        let raw = extract(SAMPLE);

        let area = symbol(&raw, "area");
        assert_eq!(area.kind, SymbolKind::Method);
        assert_eq!(area.owner.as_deref(), Some("Widget"));
        assert_eq!(area.qualified_name, "Widget::area");
    }

    #[test]
    fn a_pointer_return_keeps_its_owner_and_a_static_function_is_unexported() {
        let raw = extract(SAMPLE);

        let make = symbol(&raw, "make");
        assert_eq!(make.owner.as_deref(), Some("Factory"));
        assert!(make.exported);
        assert!(!symbol(&raw, "helper").exported);
    }

    #[test]
    fn calls_are_attributed_to_their_caller() {
        let raw = extract(SAMPLE);
        let render = id_of(&raw, "render");

        let draw = raw
            .calls
            .iter()
            .find(|call| call.callee == "draw")
            .expect("draw call");
        assert_eq!(draw.receiver.as_deref(), Some(""));
        assert_eq!(draw.caller_id, render);

        let bare_compute = raw
            .calls
            .iter()
            .find(|call| call.callee == "compute" && call.receiver.is_none())
            .expect("compute call");
        assert_eq!(bare_compute.caller_id, render);
    }

    #[test]
    fn a_member_call_names_no_type_unless_it_is_on_this() {
        let raw = extract(
            b"void f() { descs[i].init(); a.b.close(); make().run(); this->reset(); buffer.resize(1);\n\
              list->clear(); Widget::create(); (*ptr).run(); }\n",
        );

        let calls: Vec<(&str, Option<&str>)> = raw
            .calls
            .iter()
            .map(|call| (call.callee.as_str(), call.receiver.as_deref()))
            .collect();
        assert_eq!(
            calls,
            [
                ("init", Some("")),
                ("close", Some("")),
                ("run", Some("")),
                ("make", None),
                ("reset", Some("this")),
                ("resize", Some("")),
                ("clear", Some("")),
                ("Widget::create", None),
                ("run", Some("")),
            ]
        );
    }

    #[test]
    fn root_qualified_calls_and_bases_keep_the_root() {
        let raw = extract(
            b"void f() {\n  ::dgs_get_settings();\n  ::ns::helper();\n  ns::other();\n  ::tmpl<int>();\n}\n\
              class W : public ::Base {};\n",
        );

        assert_eq!(
            callees(&raw),
            ["::dgs_get_settings", "::ns::helper", "ns::other", "::tmpl"]
        );
        assert_eq!(symbol(&raw, "W").parents, ["::Base"]);
    }

    #[test]
    fn a_class_head_that_writes_its_scope_qualifies_by_it() {
        let raw = extract(
            b"class Project::Action {};\nstruct ::Root {};\ntemplate <> struct std::hash<Key> {};\n\
              namespace outer {\nclass inner::Gadget : public inner::Base {};\n}\n\
              namespace ns {\nclass ns::Knob {};\n}\n\
              namespace a {\nnamespace b {\nclass a::Dial {};\n}\n}\n",
        );

        assert_eq!(symbol(&raw, "Action").qualified_name, "Project::Action");
        assert_eq!(symbol(&raw, "Action").owner, None);
        assert_eq!(symbol(&raw, "Root").qualified_name, "Root");
        assert_eq!(symbol(&raw, "hash").qualified_name, "std::hash");
        assert_eq!(
            symbol(&raw, "Gadget").qualified_name,
            "outer::inner::Gadget"
        );
        assert_eq!(symbol(&raw, "Gadget").parents, ["inner::Base"]);
        assert_eq!(symbol(&raw, "Knob").qualified_name, "ns::Knob");
        assert_eq!(symbol(&raw, "Dial").qualified_name, "a::Dial");
    }

    #[test]
    fn parents_capture_base_class_clause() {
        let raw = extract(SAMPLE);

        assert_eq!(symbol(&raw, "Widget").parents, ["Base"]);
    }

    #[test]
    fn trailing_annotation_macros_never_name_the_symbol() {
        let raw = extract(
            b"class FS {\n  FS();\n  ~FS();\n  void load() DAG_TS_REQUIRES(mtx) { doLoad(); }\n\
              int count_ DAG_TS_GUARDED_BY(mtx);\n  int tickNumber DAG_TS_GUARDED_BY(mtx) = 0;\n\
              int size() const DAG_TS_REQUIRES(mtx) { return 0; }\n};\n\
              void free_fn() DAG_TS_REQUIRES(mtx) { work(); }\nvoid ALL_CAPS_REAL() { work(); }\n",
        );
        let names = names(&raw);

        assert!(
            !names.iter().any(|name| name.starts_with("DAG_TS_")),
            "{names:?}"
        );
        let load = symbol(&raw, "load");
        assert_eq!(load.kind, SymbolKind::Method);
        assert_eq!(load.owner.as_deref(), Some("FS"));
        assert!(names.contains(&"FS"), "{names:?}");
        assert!(names.contains(&"~FS"), "{names:?}");
        assert!(names.contains(&"ALL_CAPS_REAL"), "{names:?}");
        assert_eq!(symbol(&raw, "free_fn").kind, SymbolKind::Function);
        assert!(
            raw.callables
                .iter()
                .any(|callable| callable.key == "free_fn()"
                    && callable.role == CallableRole::Definition),
            "{:?}",
            raw.callables
        );
    }

    #[test]
    fn a_chain_of_trailing_macros_keeps_the_member() {
        let raw = extract(
            b"class T {\npublic:\n  void lock() DAG_TS_ACQUIRE() DAG_TS_NO_THREAD_SAFETY_ANALYSIS { impl.lock(); }\n\
              };\nclass U {\npublic:\n  void lock()\n    DAG_TS_ACQUIRE() DAG_TS_NO_THREAD_SAFETY_ANALYSIS\n  {\n\
              impl.lock();\n  }\n};\n\
              void unlock() DAG_TS_RELEASE() DAG_TS_NO_THREAD_SAFETY_ANALYSIS { go(); }\n",
        );
        let names = names(&raw);

        assert!(
            !names.iter().any(|name| name.starts_with("DAG_TS_")),
            "{names:?}"
        );
        assert_eq!(qualified(&raw, "T::lock").kind, SymbolKind::Method);
        assert_eq!(qualified(&raw, "U::lock").kind, SymbolKind::Method);
        assert_eq!(symbol(&raw, "unlock").kind, SymbolKind::Function);
        assert!(
            raw.callables
                .iter()
                .any(|callable| callable.key == "unlock()"
                    && callable.role == CallableRole::Definition),
            "{:?}",
            raw.callables
        );
    }

    #[test]
    fn a_macro_line_between_misparsed_members_names_no_member() {
        let raw = extract(
            b"struct FS {\n  FS() DAG_TS_ACQUIRE(mtx) { init(); }\n  MACRO_LINE()\n\
              ~FS() DAG_TS_RELEASE(mtx) { drop(); }\n};\n",
        );
        let names = names(&raw);

        assert!(!names.contains(&"MACRO_LINE"), "{names:?}");
        assert_eq!(symbol(&raw, "~FS").kind, SymbolKind::Method);
    }

    #[test]
    fn a_macro_before_the_return_type_keeps_the_function() {
        let raw = extract(
            b"JPH_INLINE float Tan(float x) { return x; }\nstatic JPH_INLINE Vec3 sMake() { return Vec3(); }\n\
              JPH_EXPORT void RegisterTypes();\nBOOL WINAPI DllMain(HINSTANCE h) { return TRUE; }\n\
              class FS {\n  int size_ DAG_TS_GUARDED_BY(mtx);\n  UINT32 count_ DAG_TS_GUARDED_BY(mtx);\n};\n",
        );

        assert_eq!(
            qualified_names(&raw),
            ["Tan", "sMake", "DllMain", "FS", "RegisterTypes"]
        );
        assert_eq!(symbol(&raw, "Tan").kind, SymbolKind::Function);
        assert!(!symbol(&raw, "sMake").exported);
        assert_eq!(
            symbol(&raw, "Tan").signature.as_deref(),
            Some("JPH_INLINE float Tan(float x)")
        );
        let keys: Vec<&str> = raw
            .callables
            .iter()
            .map(|callable| callable.key.as_str())
            .collect();
        assert!(keys.contains(&"Tan(float)"), "{keys:?}");
        assert!(keys.contains(&"RegisterTypes()"), "{keys:?}");
    }

    #[test]
    fn a_macro_between_the_return_type_and_the_name_names_no_scope() {
        let raw = extract(
            b"inline XMMATRIX XM_CALLCONV XMMatrixRotationZ(float Angle) noexcept { return M; }\n\
              GLM_FUNC_QUALIFIER GLM_CONSTEXPR vec<L, T, Q> mix(vec<L, T, Q> const& x) { return x; }\n\
              namespace ns {\nXMVECTOR XM_CALLCONV Rotate(XMVECTOR v) { return v; }\n}\n\
              int Widget::area() const { return 0; }\n\
              PUGI__FN xpath_node_set xpath_query::evaluate_node_set(xpath_node n) const { return x; }\n",
        );

        assert_eq!(
            qualified_names(&raw),
            [
                "XMMatrixRotationZ",
                "mix",
                "ns::Rotate",
                "Widget::area",
                "xpath_query::evaluate_node_set"
            ]
        );
        assert_eq!(
            symbol(&raw, "evaluate_node_set").owner.as_deref(),
            Some("xpath_query")
        );
        for name in ["XMMatrixRotationZ", "mix", "Rotate"] {
            assert_eq!(symbol(&raw, name).kind, SymbolKind::Function, "{name}");
            assert_eq!(symbol(&raw, name).owner, None, "{name}");
        }
        assert_eq!(symbol(&raw, "area").owner.as_deref(), Some("Widget"));
        let keys: Vec<&str> = raw
            .callables
            .iter()
            .map(|callable| callable.key.as_str())
            .collect();
        assert!(keys.contains(&"XMMatrixRotationZ(float)"), "{keys:?}");
    }

    #[test]
    fn trailing_macros_on_ctor_dtor_keep_the_member() {
        let raw = extract(
            b"class FS {\n  FS() DAG_TS_ACQUIRE(mtx) { init(); }\n  ~FS() DAG_TS_RELEASE(mtx) { drop(); }\n};\n\
              class LG {\n  ~LG() DAG_TS_RELEASE();\n};\n",
        );
        let names = names(&raw);

        assert!(
            !names.iter().any(|name| name.starts_with("DAG_TS_")),
            "{names:?}"
        );
        let constructor = qualified(&raw, "FS::FS");
        let destructor = qualified(&raw, "FS::~FS");
        let declared_destructor = qualified(&raw, "LG::~LG");
        assert_eq!(constructor.kind, SymbolKind::Method);
        assert_eq!(destructor.kind, SymbolKind::Method);
        assert_eq!(declared_destructor.kind, SymbolKind::Method);
    }

    #[test]
    fn templated_bases_record_their_name_without_arguments() {
        let raw = extract(
            b"class Foo : public Base<Foo> {};\n\
              class Bar : public ns::Base, protected Mixin<int>, public Plain {};\n\
              class Q : public ns::Tmpl<int> {};\n",
        );

        assert_eq!(symbol(&raw, "Foo").parents, ["Base"]);
        assert_eq!(symbol(&raw, "Bar").parents, ["ns::Base", "Mixin", "Plain"]);
        assert_eq!(symbol(&raw, "Q").parents, ["ns::Tmpl"]);
    }

    #[test]
    fn anonymous_namespace_members_are_unexported() {
        let raw = extract(HEADERISH);

        assert!(!symbol(&raw, "local_helper").exported);
    }

    #[test]
    fn new_expressions_become_constructor_calls_by_base_name() {
        let raw = extract(HEADERISH);

        let callees = callees(&raw);
        assert!(callees.contains(&"Window"), "{callees:?}");
    }

    #[test]
    fn prototypes_emit_only_without_a_same_file_definition() {
        let raw = extract(HEADERISH);

        let start = qualified(&raw, "Engine::start");
        assert_eq!(start.kind, SymbolKind::Method);
        assert_eq!(start.signature.as_deref(), Some("void start()"));
        assert!(names(&raw).contains(&"free_proto"));
        let declared_and_defined = names(&raw)
            .iter()
            .filter(|name| **name == "defined_and_declared")
            .count();
        assert_eq!(declared_and_defined, 1);
    }

    #[test]
    fn namespace_members_store_qualified_names() {
        let raw = extract(
            br"
namespace d3d {
void clearview() {}
}
namespace a {
namespace b {
void g() {}
}
}
namespace c::d {
void h() {}
}
namespace ns {
class Widget {
public:
    void area() {}
};
}
namespace {
void hidden() {}
}
void global_fn() {}
",
        );

        assert_eq!(symbol(&raw, "clearview").qualified_name, "d3d::clearview");
        assert_eq!(symbol(&raw, "clearview").name, "clearview");
        assert_eq!(symbol(&raw, "g").qualified_name, "a::b::g");
        assert_eq!(symbol(&raw, "h").qualified_name, "c::d::h");
        assert_eq!(symbol(&raw, "Widget").qualified_name, "ns::Widget");
        assert_eq!(symbol(&raw, "area").qualified_name, "Widget::area");
        assert_eq!(symbol(&raw, "area").owner.as_deref(), Some("Widget"));
        assert_eq!(symbol(&raw, "hidden").qualified_name, "hidden");
        assert_eq!(symbol(&raw, "global_fn").qualified_name, "global_fn");
    }

    #[test]
    fn namespace_prototypes_qualify_at_queue_time() {
        let raw = extract(
            br"
namespace ns {
void f();
class C {
public:
    void m(int);
};
}
void free_proto(int);
",
        );

        assert_eq!(symbol(&raw, "f").qualified_name, "ns::f");
        assert_eq!(symbol(&raw, "f").name, "f");
        assert_eq!(symbol(&raw, "m").qualified_name, "C::m");
        assert_eq!(symbol(&raw, "m").owner.as_deref(), Some("C"));
        assert_eq!(symbol(&raw, "free_proto").qualified_name, "free_proto");
    }

    #[test]
    fn entry_points_are_free_main_like_functions() {
        let raw = extract(
            br"
int main() { return 0; }

void DagorWinMain() {}

int wmain() { return 0; }

int WinMain() { return 0; }

int wWinMain() { return 0; }

void helper() {}

struct App {
    void main() {}
};
",
        );

        for name in ["main", "wmain", "WinMain", "wWinMain", "DagorWinMain"] {
            assert!(qualified(&raw, name).flags.is_entry(), "{name}");
        }
        assert!(!qualified(&raw, "helper").flags.is_entry());
        assert!(!qualified(&raw, "App::main").flags.is_entry());
        let main = qualified(&raw, "main");
        assert!(!main.flags.is_test());
        assert!(!main.flags.is_generated());
    }

    #[test]
    fn entry_requires_global_namespace_scope() {
        let raw = extract(
            br"
int main() { return 0; }

namespace app {

int WinMain() { return 0; }

namespace nested {
int wmain() { return 0; }
}

}
",
        );

        assert!(qualified(&raw, "main").flags.is_entry());
        assert!(!qualified(&raw, "app::WinMain").flags.is_entry());
        assert!(!qualified(&raw, "app::nested::wmain").flags.is_entry());
    }

    #[test]
    fn a_constructor_a_destructor_or_an_operator_is_an_entry_point() {
        let raw = extract(
            br"
struct Box {
    Box() {}
    ~Box() {}
    bool operator==(const Box &) const { return true; }
    void operatorName() {}
    void make() {}
};

Box::Box(int) {}

Box operator+(const Box &, const Box &) { return Box(); }
",
        );

        for name in ["Box::Box", "Box::~Box", "Box::operator==", "operator+"] {
            let entries: Vec<bool> = raw
                .symbols
                .iter()
                .filter(|symbol| symbol.qualified_name == name)
                .map(|symbol| symbol.flags.is_entry())
                .collect();
            assert!(
                !entries.is_empty() && !entries.contains(&false),
                "{name}: {entries:?}"
            );
        }
        assert!(!qualified(&raw, "Box::operatorName").flags.is_entry());
        assert!(!qualified(&raw, "Box::make").flags.is_entry());
        assert!(!qualified(&raw, "Box").flags.is_entry());
    }

    #[test]
    fn a_generated_banner_flags_every_symbol_of_the_file() {
        let banner =
            extract(b"// @generated by tool\nclass Emitted { int field; };\nvoid emitted() {}\n");
        let plain = extract(b"class Emitted { int field; };\nvoid emitted() {}\n");

        assert_eq!(banner.symbols.len(), 3);
        assert!(
            banner
                .symbols
                .iter()
                .all(|symbol| symbol.flags.is_generated()),
            "{:?}",
            banner.symbols
        );
        assert!(
            !plain
                .symbols
                .iter()
                .any(|symbol| symbol.flags.is_generated()),
            "{:?}",
            plain.symbols
        );
    }

    #[test]
    fn forward_declarations_and_elaborated_refs_emit_no_symbols() {
        let raw = extract(
            br"
struct Fwd;
struct Fwd { int x; };
class Widget;
template<typename T> class Vec;
class Other* ptr = nullptr;
void anchor() {}
",
        );

        assert_eq!(qualified_names(&raw), ["Fwd", "Fwd::x", "ptr", "anchor"]);
    }

    #[test]
    fn forward_declared_enums_emit_no_symbols() {
        let raw = extract(
            br"
enum class Status : int;
enum class Status : int { Ok, Err };
enum Plain : int;
void anchor() {}
",
        );

        assert_eq!(qualified_names(&raw), ["Status", "anchor"]);
        assert_eq!(symbol(&raw, "Status").kind, SymbolKind::Enum);
    }

    #[test]
    fn unions_emit_struct_symbols_with_member_attribution() {
        let raw = extract(
            br"
union U {
    int a;
    float b;
    int as_int() { return a; }
};
union Fwd;
class C {
    union { int x; };
};
",
        );

        assert_eq!(
            qualified_names(&raw),
            ["U", "U::a", "U::b", "U::as_int", "C", "C::x"]
        );
        assert_eq!(symbol(&raw, "U").kind, SymbolKind::Struct);
        assert_eq!(symbol(&raw, "as_int").owner.as_deref(), Some("U"));
    }

    #[test]
    fn type_aliases_emit_symbols() {
        let raw = extract(
            br"
typedef struct { int x; } TD;
typedef unsigned long ulong_t;
typedef int *Ptr;
typedef int Arr[10];
typedef int Multi1, Multi2;
typedef void (*Cb)(int);
using Alias = TD;
template<class T> using VecT = T;
class K {
    using value_type = int;
};
",
        );

        assert_eq!(
            qualified_names(&raw),
            [
                "TD",
                "TD::x",
                "ulong_t",
                "Ptr",
                "Arr",
                "Multi1",
                "Multi2",
                "Alias",
                "VecT",
                "K",
                "K::value_type"
            ]
        );
        assert_eq!(symbol(&raw, "TD").kind, SymbolKind::TypeAlias);
        assert_eq!(symbol(&raw, "Alias").kind, SymbolKind::TypeAlias);
        assert_eq!(symbol(&raw, "VecT").kind, SymbolKind::TypeAlias);
        let in_class_alias = symbol(&raw, "value_type");
        assert_eq!(in_class_alias.kind, SymbolKind::TypeAlias);
        assert_eq!(in_class_alias.owner.as_deref(), Some("K"));
    }

    #[test]
    fn nested_types_qualify_by_their_enclosing_class() {
        let raw = extract(
            br"
namespace host_ns {
class Host {
public:
    struct Params { int size; };
    enum class Mode { A, B };
    void run() {
        struct Local { int x; };
        using LocalAlias = int;
    }
};
class Guest {
    struct Params { int count; };
    int x = [] { struct InLambda { int a; }; return 0; }();
};
struct Params { int other; };
}
",
        );

        assert_eq!(
            qualified_names(&raw),
            [
                "host_ns::Host",
                "Host::Params",
                "Params::size",
                "Host::Mode",
                "Host::run",
                "host_ns::Local",
                "Local::x",
                "host_ns::LocalAlias",
                "host_ns::Guest",
                "Guest::Params",
                "Params::count",
                "Guest::x",
                "host_ns::InLambda",
                "InLambda::a",
                "host_ns::Params",
                "Params::other",
            ]
        );
        assert_eq!(
            qualified(&raw, "Host::Params").owner.as_deref(),
            Some("Host")
        );
        assert_eq!(qualified(&raw, "Host::Mode").owner.as_deref(), Some("Host"));
        assert_eq!(
            qualified(&raw, "Guest::Params").owner.as_deref(),
            Some("Guest")
        );
        assert_eq!(qualified(&raw, "host_ns::Local").owner, None);
        assert_eq!(qualified(&raw, "host_ns::LocalAlias").owner, None);
        assert_eq!(qualified(&raw, "host_ns::InLambda").owner, None);
        assert_eq!(qualified(&raw, "host_ns::Params").owner, None);
    }

    #[test]
    #[expect(
        clippy::too_many_lines,
        reason = "the fixture and the table of the data it declares are one fact, and split neither reads alone"
    )]
    fn namespace_scope_declarations_emit_data_symbols() {
        let raw = extract(
            br#"
namespace snd {
static constexpr int BANDS = 8;
const float kScale = 0.5f, kBias = 1.0f;
int g_count;
static int g_hidden = 0;
void (*g_hook)(int) = nullptr;
static const int kTable[] = {
    1, 2,
    3, 4,
};
int &g_ref = g_count;
extern const int
    kLimits[4];
const char *g_label = "", kSep = ',';
char *const kBuffer = nullptr;
inline constexpr int kShared = 2;
template <typename T> constexpr T kUnit = T(1);
int prototype(int);
int Holder::s_total = 0;
}
extern "C" { int c_flag; }
void body() { int local = 0; PX_ASSERT(local); }
TEST(Suite, Case) { int in_macro = 0; }
"#,
        );

        let data: Vec<(&str, SymbolKind, bool, Option<&str>)> = raw
            .symbols
            .iter()
            .filter(|symbol| matches!(symbol.kind, SymbolKind::Variable | SymbolKind::Constant))
            .map(|symbol| {
                (
                    symbol.qualified_name.as_str(),
                    symbol.kind,
                    symbol.exported,
                    symbol.signature.as_deref(),
                )
            })
            .collect();
        assert_eq!(
            data,
            [
                (
                    "snd::BANDS",
                    SymbolKind::Constant,
                    false,
                    Some("static constexpr int BANDS = 8")
                ),
                (
                    "snd::kScale",
                    SymbolKind::Constant,
                    false,
                    Some("const float kScale = 0.5f, kBias = 1.0f")
                ),
                (
                    "snd::kBias",
                    SymbolKind::Constant,
                    false,
                    Some("const float kScale = 0.5f, kBias = 1.0f")
                ),
                (
                    "snd::g_count",
                    SymbolKind::Variable,
                    true,
                    Some("int g_count")
                ),
                (
                    "snd::g_hidden",
                    SymbolKind::Variable,
                    false,
                    Some("static int g_hidden = 0")
                ),
                (
                    "snd::g_hook",
                    SymbolKind::Variable,
                    true,
                    Some("void (*g_hook)(int) = nullptr")
                ),
                (
                    "snd::kTable",
                    SymbolKind::Constant,
                    false,
                    Some("static const int kTable[]")
                ),
                (
                    "snd::g_ref",
                    SymbolKind::Variable,
                    true,
                    Some("int &g_ref = g_count")
                ),
                (
                    "snd::kLimits",
                    SymbolKind::Constant,
                    true,
                    Some("extern const int\n    kLimits[4]")
                ),
                (
                    "snd::g_label",
                    SymbolKind::Variable,
                    true,
                    Some("const char *g_label = \"\", kSep = ','")
                ),
                (
                    "snd::kSep",
                    SymbolKind::Constant,
                    false,
                    Some("const char *g_label = \"\", kSep = ','")
                ),
                (
                    "snd::kBuffer",
                    SymbolKind::Constant,
                    false,
                    Some("char *const kBuffer = nullptr")
                ),
                (
                    "snd::kShared",
                    SymbolKind::Constant,
                    true,
                    Some("inline constexpr int kShared = 2")
                ),
                (
                    "snd::kUnit",
                    SymbolKind::Constant,
                    false,
                    Some("constexpr T kUnit = T(1)")
                ),
                ("c_flag", SymbolKind::Variable, true, Some("int c_flag")),
            ]
        );
    }

    #[test]
    fn a_declaration_holding_a_parse_error_emits_no_data() {
        const SOURCE: &[u8] = b"namespace n {\nMACRO_BEGIN int x = 1;\n}\n";
        let tree = cpp_parser().parse(SOURCE, None).expect("parse");
        assert!(tree.root_node().has_error());

        let raw = extract(SOURCE);

        let data: Vec<&Symbol> = raw
            .symbols
            .iter()
            .filter(|symbol| matches!(symbol.kind, SymbolKind::Variable | SymbolKind::Constant))
            .collect();
        assert_eq!(data, Vec::<&Symbol>::new());
    }

    #[test]
    #[expect(
        clippy::too_many_lines,
        reason = "the fixture and the table of the members it declares are one fact, and split neither reads alone"
    )]
    fn field_declarations_emit_fields_of_their_class() {
        const SOURCE: &[u8] = br"
namespace ns {
int counter;
struct Config {
    int maxSources = 512;
    static constexpr int kMax = 8;
    float *weights, bias;
    int buf[8];
    const Config &parent;
    void (*callback)(int);
    void f();
    png_byte bit_depth PNG_DEPSTRUCT;
    union { int asInt; float asFloat; };
    unsigned flags : 3;
    struct { int hidden; } named;
};
}
typedef struct { int x; } TD, TD2;
void g() { int local = 0; struct Local { int y; }; }
template<> struct Traits<int> { int size; };
";
        let tree = cpp_parser().parse(SOURCE, None).expect("parse");
        assert!(tree.root_node().has_error());

        let raw = extract(SOURCE);

        let rows: Vec<(&str, SymbolKind, Option<&str>)> = raw
            .symbols
            .iter()
            .map(|symbol| {
                (
                    symbol.qualified_name.as_str(),
                    symbol.kind,
                    symbol.signature.as_deref(),
                )
            })
            .collect();
        assert_eq!(
            rows,
            [
                ("ns::counter", SymbolKind::Variable, Some("int counter")),
                ("ns::Config", SymbolKind::Struct, None),
                (
                    "Config::maxSources",
                    SymbolKind::Field,
                    Some("int maxSources = 512")
                ),
                (
                    "Config::kMax",
                    SymbolKind::Field,
                    Some("static constexpr int kMax = 8")
                ),
                (
                    "Config::weights",
                    SymbolKind::Field,
                    Some("float *weights, bias")
                ),
                (
                    "Config::bias",
                    SymbolKind::Field,
                    Some("float *weights, bias")
                ),
                ("Config::buf", SymbolKind::Field, Some("int buf[8]")),
                (
                    "Config::parent",
                    SymbolKind::Field,
                    Some("const Config &parent")
                ),
                (
                    "Config::callback",
                    SymbolKind::Field,
                    Some("void (*callback)(int)")
                ),
                ("Config::asInt", SymbolKind::Field, Some("int asInt")),
                ("Config::asFloat", SymbolKind::Field, Some("float asFloat")),
                (
                    "Config::flags",
                    SymbolKind::Field,
                    Some("unsigned flags : 3")
                ),
                (
                    "Config::named",
                    SymbolKind::Field,
                    Some("struct { int hidden; } named")
                ),
                ("TD", SymbolKind::TypeAlias, None),
                ("TD2", SymbolKind::TypeAlias, None),
                ("TD::x", SymbolKind::Field, Some("int x")),
                ("g", SymbolKind::Function, Some("void g()")),
                ("Local", SymbolKind::Struct, None),
                ("Local::y", SymbolKind::Field, Some("int y")),
                ("Traits", SymbolKind::Struct, None),
                ("Traits::size", SymbolKind::Field, Some("int size")),
                ("Config::f", SymbolKind::Method, Some("void f()")),
            ]
        );
        assert_eq!(
            qualified(&raw, "Config::maxSources").owner.as_deref(),
            Some("Config")
        );
    }

    #[test]
    fn member_template_calls_strip_template_arguments() {
        let raw = extract(
            br"
class Widget {
public:
    template<typename T> void method(T x) {}
};
void F() {
    Widget w;
    w.method<int>(42);
}
",
        );

        let calls: Vec<(&str, Option<&str>)> = raw
            .calls
            .iter()
            .map(|call| (call.callee.as_str(), call.receiver.as_deref()))
            .collect();
        assert_eq!(calls, [("method", Some(""))]);
    }

    #[test]
    fn templated_casts_emit_no_calls() {
        let raw = extract(
            br"
void F() {
    int x = 0;
    double d = static_cast<double>(x);
    void* p = reinterpret_cast<void*>(&x);
    const int& r = const_cast<const int&>(x);
    F* f = dynamic_cast<F*>(p);
    doThing<int>(4);
}
",
        );

        assert_eq!(callees(&raw), ["doThing"]);
    }

    #[test]
    fn template_disambiguator_member_calls_reduce_to_bare_name() {
        let raw = extract(
            br"
void F() {
    Widget w;
    w.template get<int>();
}
",
        );

        let calls: Vec<(&str, Option<&str>)> = raw
            .calls
            .iter()
            .map(|call| (call.callee.as_str(), call.receiver.as_deref()))
            .collect();
        assert_eq!(calls, [("get", Some(""))]);
    }

    #[test]
    fn static_member_functions_stay_exported() {
        let raw = extract(
            br"
class Foo {
public:
    static int inline_def() { return 1; }
    static int proto_only();
};

static int free_helper() { return 2; }
",
        );

        assert!(qualified(&raw, "Foo::inline_def").exported);
        assert!(qualified(&raw, "Foo::proto_only").exported);
        assert!(!qualified(&raw, "free_helper").exported);
    }

    #[test]
    fn a_call_before_the_first_symbol_is_not_recorded() {
        let raw = extract(b"TEST(Suite, Case) { helper(); }\nvoid run() { go(); }\n");

        assert_eq!(callees(&raw), ["go"]);
    }

    #[test]
    fn includes_record_their_specifier_and_form() {
        let raw = extract(b"#include \"core/util.h\"\n#include <vector>\n#include SOME_MACRO\n");

        let directives: Vec<(u32, &str, u32)> = raw
            .directives
            .iter()
            .map(|directive| (directive.form, directive.spec.as_str(), directive.line))
            .collect();
        assert_eq!(
            directives,
            [
                (crate::QUOTED_INCLUDE, "core/util.h", 1),
                (crate::BRACKET_INCLUDE, "vector", 2)
            ]
        );
    }
}
