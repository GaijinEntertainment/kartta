use std::collections::HashSet;
use std::ops::Range;

use kartta_lang::tree::{line, text};
use kartta_lang::{FILE_SCOPE, Receiver, SymbolId, SymbolKind, UseRef};
use tree_sitter::Node;

use super::{Emitter, USE_FORM};

// constraint: a read waits for the end of the walk, because a name binds only what its own file declares or its `use`s
// import, which a definition or a `use` below the read can still add; `scope` indexes the interned chain of the
// inline module a bare read is in, or the type a read through `Self` is in; `through_module` indexes the place a read
// through a path holds for the module its last segment may name, should no type of the file answer it
pub(super) struct ValueRead {
    name: Range<usize>,
    receiver: Receiver,
    scope: usize,
    site_of: SymbolId,
    line: u32,
    through_module: Option<usize>,
    module: Option<usize>,
    out_of_use_scope: bool,
}

impl Emitter<'_> {
    pub(super) fn name(&mut self, node: Node<'_>, parent: Option<(&str, Option<&str>)>) {
        if parent.is_some_and(|(parent, field)| reads_value(parent, field))
            && !self.is_bound(&self.source[node.byte_range()])
        {
            self.bare_read(node);
        }
    }

    pub(super) fn bare_read(&mut self, node: Node<'_>) {
        let chain = self.modules.last().map(|module| module.chain.as_str());
        let scope = intern(&mut self.read_scopes, chain.unwrap_or_default());
        self.defer_read(node, (Receiver::Bare, None), scope);
    }

    pub(super) fn scoped_read(&mut self, node: Node<'_>, parent: Option<(&str, Option<&str>)>) {
        if !parent.is_some_and(|(parent, field)| reads_value(parent, field)) {
            return;
        }
        let (Some(path), Some(name)) = (
            node.child_by_field_name("path"),
            node.child_by_field_name("name")
                .filter(|name| name.kind() == "identifier"),
        ) else {
            return;
        };
        self.path_read(text(path, self.source), name);
    }

    // constraint: `Self::f` read as a value names a member of the type around it, and `T::f` one of `T` as a call
    // through `T` names it
    pub(super) fn path_read(&mut self, written: String, name: Node<'_>) {
        if written != "Self" {
            let at = (name.start_byte(), line(name.start_position().row));
            let through_module = self.record_read_path(&written, at);
            let path = Receiver::Path(self.module_path(&written).unwrap_or(written));
            self.defer_read(name, (path, through_module), 0);
        } else if let Some(own) = self.types.last() {
            let scope = intern(&mut self.read_scopes, &own.name);
            self.defer_read(name, (Receiver::Own, None), scope);
        }
    }

    fn defer_read(
        &mut self,
        name: Node<'_>,
        (receiver, through_module): (Receiver, Option<usize>),
        scope: usize,
    ) {
        self.value_reads.push(ValueRead {
            name: name.byte_range(),
            receiver,
            scope,
            site_of: self.functions.last().map_or(FILE_SCOPE, |scope| scope.id),
            line: line(name.start_position().row),
            through_module,
            module: self.modules.last().map(|module| module.end),
            out_of_use_scope: false,
        });
    }

    // constraint: a `use` inside a function body or an inline module binds its name in that block alone, as a bare call
    // of the name is read, so a bare read outside every block whose `use` binds the name, with no `use` of it outside
    // them, takes no binding from it
    pub(super) fn scope_bare_reads(&mut self) {
        if self.value_reads.is_empty() || self.imports.iter().all(|import| !import.in_block) {
            return;
        }
        let (_, at_file_level) = self.imported_names();
        let out: Vec<usize> = (0..)
            .zip(&self.value_reads)
            .filter(|(_, read)| {
                let name = String::from_utf8_lossy(&self.source[read.name.clone()]);
                matches!(read.receiver, Receiver::Bare)
                    && !at_file_level.contains(name.as_ref())
                    && !self.in_use_scope(&name, read.name.start, read.module)
            })
            .map(|(at, _)| at)
            .collect();
        for at in out {
            self.value_reads[at].out_of_use_scope = true;
        }
    }

    // constraint: a bare read binds an item its own file declares outside every owner and body, or one a `use` or a
    // glob of the file imports, as a bare call of the name does, so the walk keeps one only where its own inline
    // module declares the item, a kept `use` in scope of it binds its name or the file holds a glob, a read through a
    // module's path only where the file keeps the path of its last segment, and a read through `Self` or a type only
    // where the file declares a member of the name under that type; any other is dropped before it costs a use, which
    // leaves out a member that only a file it includes declares
    pub(super) fn bind_value_reads(&mut self) {
        if self.value_reads.is_empty() {
            return;
        }
        let local: HashSet<u32> = self.raw.local_symbols.iter().map(|id| id.0).collect();
        let mut free = HashSet::new();
        let mut members = HashSet::new();
        for (at, symbol) in (0..).zip(&self.raw.symbols) {
            match &symbol.owner {
                Some(type_name) => {
                    members.insert((type_name.as_str(), symbol.name.as_str()));
                }
                None if symbol.kind != SymbolKind::Macro && !local.contains(&at) => {
                    let qualified = symbol.qualified_name.as_str();
                    free.insert(qualified.rsplit_once("::").unwrap_or(("", qualified)));
                }
                None => {}
            }
        }
        let (source, scopes, directives) = (self.source, &self.read_scopes, &self.raw.directives);
        let written = &self.written_paths;
        let globbed = !self.globs.is_empty();
        let bound = |name: &str| {
            (directives.iter()).any(|directive| {
                directive.form == USE_FORM
                    && directive.names.first().is_some_and(|held| held == name)
            })
        };
        self.raw
            .uses
            .extend(self.value_reads.drain(..).filter_map(|read| {
                let name = String::from_utf8_lossy(&source[read.name]);
                let scope = scopes[read.scope].as_str();
                let receiver = match read.receiver {
                    Receiver::Path(type_name)
                        if read.through_module.is_some_and(|at| !written[at].refused)
                            && !members.contains(&(type_name.as_str(), name.as_ref())) =>
                    {
                        let module = type_name
                            .rsplit_once("::")
                            .map_or(type_name.as_str(), |(_, last)| last);
                        Receiver::Qualifier(module.to_owned())
                    }
                    receiver => receiver,
                };
                let known = match &receiver {
                    Receiver::Bare => {
                        free.contains(&(scope, name.as_ref()))
                            || globbed
                            || (!read.out_of_use_scope && bound(&name))
                    }
                    Receiver::Qualifier(qualifier) => bound(qualifier),
                    Receiver::Own => members.contains(&(scope, name.as_ref())),
                    Receiver::Path(type_name) => {
                        members.contains(&(type_name.as_str(), name.as_ref()))
                    }
                    _ => false,
                };
                known.then(|| UseRef::new(read.site_of, name.into_owned(), receiver, read.line))
            }));
    }
}

// constraint: the reads of one module or type come in a run, so one copy of its name serves the run; the root module
// is the scope at 0
fn intern(scopes: &mut Vec<String>, scope: &str) -> usize {
    if scope.is_empty() {
        return 0;
    }
    if scopes.last().is_none_or(|last| last != scope) {
        scopes.push(scope.to_owned());
    }
    scopes.len() - 1
}

// constraint: a name is read as a value as an argument, an element, an operand, a range's bound, a condition, an
// indexed value or an index, a returned, a broken-out, a block's or a closure's last value, the value a `let`, a `let`
// condition, an assignment, a `const`, a `static`, a field, a reference, a cast, a `match`, a match arm or a `for`
// takes, either side of `+=` and its kin, the base of a field access or a method call, an awaited or a `?` operand,
// and a field written shorthand
fn reads_value(parent: &str, field: Option<&str>) -> bool {
    match parent {
        "arguments"
        | "array_expression"
        | "tuple_expression"
        | "binary_expression"
        | "unary_expression"
        | "range_expression"
        | "parenthesized_expression"
        | "return_expression"
        | "break_expression"
        | "index_expression"
        | "compound_assignment_expr"
        | "await_expression"
        | "try_expression"
        | "block"
        | "let_chain"
        | "shorthand_field_initializer" => true,
        "let_declaration"
        | "let_condition"
        | "const_item"
        | "static_item"
        | "field_initializer"
        | "field_expression"
        | "reference_expression"
        | "type_cast_expression"
        | "match_expression"
        | "match_arm"
        | "for_expression" => field == Some("value"),
        "if_expression" | "while_expression" => field == Some("condition"),
        "closure_expression" => field == Some("body"),
        "assignment_expression" => field == Some("right"),
        _ => false,
    }
}
