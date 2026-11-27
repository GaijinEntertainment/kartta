use std::borrow::Cow;
use std::cell::OnceCell;
use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet};

use kartta_lang::tree::{line, named_child_nodes, text};
use kartta_lang::{ImplementsAssertion, RawDirective, Symbol, SymbolKind};
use tree_sitter::Node;

use super::syntax::path_text;
use super::{Emitter, GLOB_FORM, MOD_FORM, MOD_TEST_FORM, USE_FORM};

// constraint: a name a `use` binds, its directive, whether the `use` is inside a function body or an inline module,
// whether inside a function body, which no signature sees, and the chain of the inline module around it, whose
// modules its first segment can name
pub(super) struct Import {
    pub(super) directive: usize,
    pub(super) in_block: bool,
    pub(super) in_body: bool,
    pub(super) module: Option<String>,
}

// constraint: a path call's site, or none for a path a value read is written through, the byte it starts at and the
// inline module around it, tells which `use` inside a function body or an inline module is in scope there
pub(super) struct WrittenPath {
    pub(super) refused: bool,
    call: Option<usize>,
    directive: usize,
    at: usize,
    module: Option<usize>,
}

impl Emitter<'_> {
    pub(super) fn record_import(&mut self, node: Node<'_>) {
        let Some(argument) = node.child_by_field_name("argument") else {
            return;
        };
        let first_directive = self.raw.directives.len();
        self.use_paths(argument, line(node.start_position().row));
        if self.in_a_block()
            && let Some(block) = node.parent()
        {
            self.scope_use_to_block(block.byte_range(), first_directive);
        }
    }

    // constraint: a `use` inside a function body or an inline module is in scope in its block alone, the body of the
    // function or of the module; one outside both is in scope in the whole file
    const fn in_a_block(&self) -> bool {
        !self.functions.is_empty() || !self.modules.is_empty()
    }

    fn use_paths(&mut self, node: Node<'_>, line: u32) {
        let mut prefix = String::new();
        let mut waiting = vec![(node, 0)];
        while let Some((node, prefix_end)) = waiting.pop() {
            prefix.truncate(prefix_end);
            match node.kind() {
                "scoped_use_list" => {
                    extend_prefix(&mut prefix, node, self.source);
                    let list = node.child_by_field_name("list");
                    waiting.extend(list.map(|list| (list, prefix.len())));
                }
                "use_list" => {
                    let items = named_child_nodes(node).rev();
                    waiting.extend(items.map(|item| (item, prefix_end)));
                }
                _ => self.use_path(node, &prefix, line),
            }
        }
    }

    // constraint: a `use` binds each name its tree ends in, an alias where one is written, and `self` in a list the
    // module the list is under; a glob binds no name it spells, and `as _` binds none
    fn use_path(&mut self, node: Node<'_>, prefix: &str, line: u32) {
        let joined = |path: &str| {
            if prefix.is_empty() {
                path.to_owned()
            } else {
                format!("{prefix}::{path}")
            }
        };
        let source = self.source;
        match node.kind() {
            "identifier" | "scoped_identifier" => {
                if let Some(path) = path_text(node, source) {
                    self.push_import(last(&path).to_owned(), joined(&path), line);
                }
            }
            "self" if !prefix.is_empty() => {
                self.push_import(last(prefix).to_owned(), prefix.to_owned(), line);
            }
            "use_as_clause" => {
                let alias = node
                    .child_by_field_name("alias")
                    .map(|alias| text(alias, source));
                let path = node
                    .child_by_field_name("path")
                    .and_then(|path| path_text(path, source));
                if let (Some(alias), Some(path)) = (alias, path)
                    && alias != "_"
                {
                    self.push_import(alias, joined(&path), line);
                }
            }
            "use_wildcard" if !self.in_a_block() => {
                let path = named_child_nodes(node)
                    .next()
                    .and_then(|path| path_text(path, source))
                    .map_or_else(|| prefix.to_owned(), |path| joined(&path));
                self.push_glob(path, line);
            }
            _ => {}
        }
    }

    // constraint: a glob outside every function body and inline module binds the items of the module it ends at for
    // the whole file, as one directive; the file's bindings cannot scope one to a block, and a glob of a type, its
    // variants or members, of the standard library or of a path that leaves the tree binds no module of it
    fn push_glob(&mut self, mut path: String, line: u32) {
        if path.starts_with("::") {
            path.drain(.."::".len());
        }
        if !names_a_module(head(&path)) || names_a_type(last(&path)) {
            return;
        }
        self.globs.push(self.raw.directives.len());
        self.raw
            .directives
            .push(RawDirective::new(GLOB_FORM, path, line));
    }

    // constraint: each name a `use` binds holds its place among the file's directives until the walk ends and tells
    // whether it is kept, so the kept ones stay in source order
    fn push_import(&mut self, name: String, mut path: String, line: u32) {
        if path.starts_with("::") {
            path.drain(.."::".len());
        } else if self.starts_inside_the_file(&path) {
            path.clear();
        }
        self.imports.push(Import {
            directive: self.raw.directives.len(),
            in_block: self.in_a_block(),
            in_body: !self.functions.is_empty(),
            module: self.modules.last().map(|module| module.chain.clone()),
        });
        let mut directive = RawDirective::new(USE_FORM, path, line);
        directive.names = vec![name];
        self.raw.directives.push(directive);
    }

    // constraint: a path call, the path of its receiver, or a macro invocation, the path of its macro, holds a place
    // for the resolver only where its path may start at another crate, at the caller's crate or module, or at a name a
    // `use` binds, a name that is neither a type, written capitalised, nor the standard library's crates
    pub(super) fn record_written_path(&mut self, index: usize, written: String, at: usize) {
        let line = self.raw.calls[index].line;
        self.push_written_path(Some(index), written, (at, line));
    }

    // constraint: a value read through a path that may end at a module, not at a capitalised item, holds a place as
    // a path call's receiver does, whose binding the read's last segment takes; a path from the root is left out, as
    // a macro's arguments leave it out
    pub(super) fn record_read_path(
        &mut self,
        written: &str,
        (at, line): (usize, u32),
    ) -> Option<usize> {
        let held = !written.starts_with("::")
            && !names_a_type(last(written))
            && self.push_written_path(None, written.to_owned(), (at, line));
        held.then(|| self.written_paths.len() - 1)
    }

    fn push_written_path(
        &mut self,
        call: Option<usize>,
        mut written: String,
        (at, line): (usize, u32),
    ) -> bool {
        if written.starts_with("::") {
            written.drain(..2);
        }
        let holds = names_a_module(head(&written)) && !self.starts_inside_the_file(&written);
        if holds {
            self.written_paths.push(WrittenPath {
                refused: false,
                call,
                directive: self.raw.directives.len(),
                at,
                module: self.modules.last().map(|module| module.end),
            });
            self.raw
                .directives
                .push(RawDirective::new(USE_FORM, written, line));
        }
        holds
    }

    // constraint: the resolver walks a path through `self::` or `super::` from the file's own module, so inside an
    // inline module such a path keeps none, and a `use` of one binds its name to the empty path, which keeps none
    fn starts_inside_the_file(&self, path: &str) -> bool {
        !self.modules.is_empty() && matches!(head(path), "self" | "super")
    }

    // constraint: a name the file binds through another crate keeps the place of the first directive that binds it,
    // with the path it reaches; a name the file declares at its top level, or binds to two paths, is the file's own or
    // cannot be told apart, so every directive of it is dropped, but for a `use` of a name the file declares only as a
    // module, and a path call through a module file the file declares, which the path names
    pub(super) fn record_imports(&mut self) {
        if !self.imports.is_empty() || !self.globs.is_empty() {
            self.start_at_declared_modules();
        }
        self.assert_undeclared_impls();
        if self.imports.is_empty() && self.written_paths.is_empty() {
            return;
        }
        self.reach_own_names_through_their_module();
        let (mut kept, refused) = self.kept_paths();
        for at in refused {
            self.written_paths[at].refused = true;
        }
        self.scope_bare_reads();
        let mut index = 0;
        self.raw.directives.retain_mut(|directive| {
            let at = index;
            index += 1;
            if directive.form != USE_FORM {
                return true;
            }
            let Some(path) = kept.remove(&at) else {
                return false;
            };
            if directive.names.is_empty() {
                directive.names.push(last(&directive.spec).to_owned());
            }
            if let Some(path) = path {
                directive.spec = path;
            }
            true
        });
    }

    // constraint: an impl of a type the file does not declare is an assertion whose two names the resolver reads, each
    // the path it reaches, or a bare name the file's own scope reads, which a declaration or a glob may answer; a name
    // that neither reaches is the prelude's, a macro's or the standard library's, so the impl says nothing of the tree;
    // a path whose last name the file also declares says nothing either, as its name alone would read as the file's own
    fn assert_undeclared_impls(&mut self) {
        let pairs = std::mem::take(&mut self.pending_parents);
        if pairs.is_empty() {
            return;
        }
        let (imported, _) = self.imported_names();
        let module_files = self.module_files();
        let declared: HashSet<&str> = (self.raw.symbols.iter())
            .filter(|symbol| symbol.kind.is_type())
            .map(|symbol| symbol.name.as_str())
            .collect();
        let reached = |written: &str| {
            let (first, rest) = written.split_once("::").unwrap_or((written, ""));
            let path = match imported.get(first) {
                Some(Some((path, _))) if rest.is_empty() => (*path).to_owned(),
                Some(Some((path, _))) => format!("{path}::{rest}"),
                Some(None) => return None,
                None if matches!(first, "crate" | "self" | "super") => written.to_owned(),
                None if rest.is_empty() => {
                    let in_scope = declared.contains(written) || !self.globs.is_empty();
                    return in_scope.then(|| written.to_owned());
                }
                None if module_files.contains(first) => format!("self::{written}"),
                None => written.trim_start_matches("::").to_owned(),
            };
            (names_a_module(head(&path)) && !self.starts_inside_the_file(&path)).then_some(path)
        };
        let assertions = (pairs.iter())
            .filter(|(type_name, _)| {
                !type_name.contains("::") || !declared.contains(last(type_name))
            })
            .filter_map(|(type_name, trait_name)| {
                Some(ImplementsAssertion::new(
                    reached(type_name)?,
                    reached(trait_name)?,
                ))
            })
            .collect();
        self.raw.assertions = assertions;
    }

    // constraint: a path call through `crate` or `super` that then ends at a name the file declares at its top level,
    // `super::new()` beside `fn new`, cannot bind the name, which is the file's own; it takes the path's last keyword
    // as its receiver, bound to the module the path names, so the call reaches that module's item; `self::new()`
    // names the file's own item, which the name already is
    fn reach_own_names_through_their_module(&mut self) {
        let directives = &self.raw.directives;
        let calls = &self.raw.calls;
        let mut through: Vec<(usize, usize, usize)> = (self.written_paths.iter())
            .filter_map(|site| {
                let index = site.call?;
                let call = &calls[index];
                let (module, name) = directives[site.directive].spec.rsplit_once("::")?;
                (call.receiver.is_none()
                    && name == call.callee
                    && module != "self"
                    && module
                        .split("::")
                        .all(|segment| matches!(segment, "crate" | "self" | "super")))
                .then_some((index, site.directive, module.len()))
            })
            .collect();
        if through.is_empty() {
            return;
        }
        let declared = self.declared_names();
        through.retain(|&(call, _, _)| declared.contains_key(calls[call].callee.as_str()));
        for (call, directive, module) in through {
            let spec = &mut self.raw.directives[directive].spec;
            spec.truncate(module);
            self.raw.calls[call].receiver = Some(last(spec).to_owned());
        }
    }

    // constraint: a path call's receiver, the last segment of its path, is bound to the path where the receiver did not
    // settle on a declaration of the file, the path's first segment read through the file's `use` of that name, or,
    // outside every inline module, started at the file's own module where it is a module file the file declares, and a
    // macro invocation's path, which records no receiver, binds the macro's name the same way; a `use`
    // inside a function body or an inline module is in scope in its block alone, so where a name only such `use`s bind
    // is out of their scope, a path from it reads as written, and a bare call of it leaves the name no path, as two
    // paths do
    fn kept_paths(&self) -> (HashMap<usize, Option<String>>, Vec<usize>) {
        let directives = &self.raw.directives;
        let declared = self.declared_names();
        let (imported, at_file_level) = self.imported_names();
        let out_of_scope = |name: &str, at: usize, module: Option<usize>| {
            imported.contains_key(name)
                && !at_file_level.contains(name)
                && !self.in_use_scope(name, at, module)
        };
        let module_files = OnceCell::new();
        let pulls_in = |name: &str| {
            module_files
                .get_or_init(|| self.module_files())
                .contains(name)
        };
        let mut bound: HashMap<&str, Bound<Cow<'_, str>>> = (imported.iter())
            .map(|(&name, held)| (name, held.map(|(path, at)| (Cow::Borrowed(path), at))))
            .collect();
        let reached = |site: &WrittenPath| {
            let written = directives[site.directive].spec.as_str();
            let (first, rest) = written.split_once("::").unwrap_or((written, ""));
            let head = if out_of_scope(first, site.at, site.module) {
                None
            } else {
                imported.get(first)
            };
            match head {
                Some(Some((path, _))) if rest.is_empty() => Some(Cow::Borrowed(*path)),
                Some(Some((path, _))) => Some(Cow::Owned(format!("{path}::{rest}"))),
                Some(None) => None,
                None if site.module.is_none() && pulls_in(first) => {
                    Some(Cow::Owned(format!("self::{written}")))
                }
                None if declared.contains_key(first) => None,
                None => Some(Cow::Borrowed(written)),
            }
        };
        let calls = (self.written_paths.iter())
            .filter_map(|site| Some((&self.raw.calls[site.call?], site)));
        for (call, site) in calls {
            let receiver = last(&directives[site.directive].spec);
            if call.receiver.as_deref().unwrap_or(&call.callee) == receiver
                && let Some(path) = reached(site)
            {
                bind(&mut bound, receiver, path, site.directive);
            }
        }
        let reads = (0..)
            .zip(&self.written_paths)
            .filter(|(_, site)| site.call.is_none());
        let refused = bind_read_paths(&mut bound, reads, directives, reached);
        for (call, at, module) in self.bare_block_calls() {
            let callee = self.raw.calls[call].callee.as_str();
            if out_of_scope(callee, at, module) {
                bound.insert(callee, None);
            }
        }
        (self.kept_bindings(bound, &declared, pulls_in), refused)
    }

    // constraint: a bound name keeps its first directive where its path starts in the tree, a capitalised path only
    // where the file calls the name bare, and a name the file declares keeps none but a module's
    fn kept_bindings<'d>(
        &'d self,
        bound: HashMap<&'d str, Bound<Cow<'d, str>>>,
        declared: &HashMap<&str, bool>,
        pulls_in: impl Fn(&str) -> bool,
    ) -> HashMap<usize, Option<String>> {
        let directives = &self.raw.directives;
        let bare_calls = OnceCell::new();
        let called_bare = |name: &str| {
            bare_calls
                .get_or_init(|| {
                    (self.raw.calls.iter())
                        .filter(|call| call.receiver.is_none())
                        .map(|call| call.callee.as_str())
                        .collect::<HashSet<_>>()
                })
                .contains(name)
        };
        (bound.into_iter())
            .filter_map(|(name, held)| {
                let (path, at) = held?;
                let keeps = names_a_module(head(&path))
                    && (!names_a_type(last(&path)) || name != last(&path) || called_bare(name));
                let own = declared.get(name).is_some_and(|&only_modules| {
                    !only_modules || (directives[at].names.is_empty() && !pulls_in(name))
                });
                (!own && keeps).then(|| {
                    let changed = (path != directives[at].spec.as_str()).then(|| path.into_owned());
                    (at, changed)
                })
            })
            .collect()
    }

    // constraint: a file's bindings hold one path per name, so a `use` inside a function body or an inline module binds
    // its names for the whole file too, and two paths of a name anywhere leave it none; the names a `use` outside every
    // body binds, in scope everywhere, are set apart
    pub(super) fn imported_names(&self) -> (HashMap<&str, Bound<&str>>, HashSet<&str>) {
        let mut imported = HashMap::new();
        let mut at_file_level = HashSet::new();
        for import in &self.imports {
            let directive = &self.raw.directives[import.directive];
            let name = directive.names[0].as_str();
            bind(
                &mut imported,
                name,
                directive.spec.as_str(),
                import.directive,
            );
            if !import.in_block {
                at_file_level.insert(name);
            }
        }
        (imported, at_file_level)
    }

    // constraint: since the 2018 edition a path's first segment names an item of the module the path is written in
    // before it names a crate, so a `use` or a glob whose first segment is a module the file declares there starts at
    // the file's own module, `self::`, which inside an inline module keeps nothing, as a `self::` path there does
    fn start_at_declared_modules(&mut self) {
        let modules: HashSet<&str> = self
            .file_items()
            .filter(|symbol| symbol.kind == SymbolKind::Namespace)
            .map(|symbol| symbol.qualified_name.as_str())
            .collect();
        if modules.is_empty() {
            return;
        }
        let directives = &self.raw.directives;
        let declares = |spec: &str, module: Option<&str>| {
            let first = head(spec);
            module.map_or_else(
                || modules.contains(first),
                |chain| modules.contains(format!("{chain}::{first}").as_str()),
            )
        };
        let starts: Vec<(usize, bool)> = (self.imports.iter())
            .filter(|import| declares(&directives[import.directive].spec, import.module.as_deref()))
            .map(|import| (import.directive, import.module.is_none()))
            .chain(
                (self.globs.iter())
                    .filter(|&&at| declares(&directives[at].spec, None))
                    .map(|&at| (at, true)),
            )
            .collect();
        for (at, at_the_root) in starts {
            let spec = &mut self.raw.directives[at].spec;
            if at_the_root {
                spec.insert_str(0, "self::");
            } else {
                spec.clear();
            }
        }
    }

    // constraint: a name declared at the file's top level maps to whether every such declaration is a module, which
    // Rust keeps apart from the functions and types a `use` of the name can bind
    fn declared_names(&self) -> HashMap<&str, bool> {
        let mut declared = HashMap::new();
        for symbol in self.file_items() {
            if symbol.qualified_name == symbol.name {
                let only_modules = declared.entry(symbol.name.as_str()).or_insert(true);
                *only_modules &= symbol.kind == SymbolKind::Namespace;
            }
        }
        declared
    }

    // constraint: the modules a bodiless `mod NAME;` outside every inline module declares, whose file the resolver
    // finds from the file's own module; one a `#[path]` places is in a file the resolver does not walk to, and an
    // inline module's items are the file's own, which the same-file tiers bind
    fn module_files(&self) -> HashSet<&str> {
        (self.raw.directives.iter())
            .filter(|directive| {
                matches!(directive.form, MOD_FORM | MOD_TEST_FORM)
                    && directive.names.is_empty()
                    && !directive.spec.contains('/')
            })
            .map(|directive| directive.spec.as_str())
            .collect()
    }

    fn file_items(&self) -> impl Iterator<Item = &Symbol> {
        let local: HashSet<u32> = self.raw.local_symbols.iter().map(|id| id.0).collect();
        (0..)
            .zip(&self.raw.symbols)
            .filter(move |(id, symbol)| symbol.owner.is_none() && !local.contains(id))
            .map(|(_, symbol)| symbol)
    }
}

// constraint: the path a name is bound to and the first directive that binds it, or `None` for a name bound to two
// paths
type Bound<P> = Option<(P, usize)>;

// constraint: a value read's path binds its last segment only where no `use` and no path call holds that name on
// another path, so a read takes no binding a call relies on; each read whose path binds nothing is refused, by its
// place among the written paths
fn bind_read_paths<'d>(
    bound: &mut HashMap<&'d str, Bound<Cow<'d, str>>>,
    reads: impl Iterator<Item = (usize, &'d WrittenPath)>,
    directives: &'d [RawDirective],
    reached: impl Fn(&WrittenPath) -> Option<Cow<'d, str>>,
) -> Vec<usize> {
    let mut refused = Vec::new();
    for (at, site) in reads {
        let receiver = last(&directives[site.directive].spec);
        match (reached(site), bound.get(receiver)) {
            (Some(path), Some(Some((held, _)))) if *held == path => {
                bind(bound, receiver, path, site.directive);
            }
            (Some(path), None) => bind(bound, receiver, path, site.directive),
            _ => refused.push(at),
        }
    }
    refused
}

fn bind<'a, P: PartialEq>(
    bound: &mut HashMap<&'a str, Bound<P>>,
    name: &'a str,
    path: P,
    at: usize,
) {
    match bound.entry(name) {
        Entry::Vacant(vacant) => {
            vacant.insert(Some((path, at)));
        }
        Entry::Occupied(mut occupied) => match occupied.get_mut() {
            Some((held, first)) if *held == path => *first = (*first).min(at),
            Some(_) => {
                occupied.insert(None);
            }
            None => {}
        },
    }
}

fn extend_prefix(prefix: &mut String, scoped_list: Node<'_>, source: &[u8]) {
    if !prefix.is_empty() {
        prefix.push_str("::");
    }
    let path = scoped_list
        .child_by_field_name("path")
        .and_then(|path| path_text(path, source));
    prefix.push_str(path.as_deref().unwrap_or_default());
}

fn head(path: &str) -> &str {
    path.split_once("::").map_or(path, |(first, _)| first)
}

fn last(path: &str) -> &str {
    path.rsplit_once("::").map_or(path, |(_, last)| last)
}

// constraint: a path that ends at a capitalised item, a type or a function named so, binds only a bare call of its
// name, as a tuple struct's constructor is, since a call through it names a member of the type rather than an item of
// the module that holds it; so it is kept only where the file calls the name bare
fn names_a_type(item: &str) -> bool {
    item.starts_with(|first: char| first.is_ascii_uppercase())
}

// constraint: a path starts at a module of the tree where its first segment names a crate other than the standard
// library's, or the caller's own crate or module through `crate`, `self` or `super`
fn names_a_module(head: &str) -> bool {
    head.starts_with(|first: char| first.is_ascii_lowercase() || first == '_')
        && !matches!(head, "std" | "core" | "alloc" | "_")
}
