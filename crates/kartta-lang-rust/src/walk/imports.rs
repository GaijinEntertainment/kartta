use std::borrow::Cow;
use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet};

use kartta_lang::RawDirective;
use kartta_lang::tree::{line, named_child_nodes, text};
use tree_sitter::Node;

use super::syntax::path_text;
use super::{Emitter, USE_FORM};

impl Emitter<'_> {
    pub(super) fn record_import(&mut self, node: Node<'_>) {
        let Some(argument) = node.child_by_field_name("argument") else {
            return;
        };
        self.use_paths(argument, "", line(node.start_position().row));
    }

    // constraint: a `use` binds each name its tree ends in, an alias where one is written, and `self` in a list the
    // module the list is under; a glob binds no name it spells, and `as _` binds none
    fn use_paths(&mut self, node: Node<'_>, prefix: &str, line: u32) {
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
            "scoped_use_list" => {
                let path = node
                    .child_by_field_name("path")
                    .and_then(|path| path_text(path, source))
                    .unwrap_or_default();
                if let Some(list) = node.child_by_field_name("list") {
                    self.use_paths(list, &joined(&path), line);
                }
            }
            "use_list" => {
                for item in named_child_nodes(node) {
                    self.use_paths(item, prefix, line);
                }
            }
            _ => {}
        }
    }

    // constraint: each name a `use` binds holds its place among the file's directives until the walk ends and tells
    // whether it is kept, so the kept ones stay in source order
    fn push_import(&mut self, name: String, mut path: String, line: u32) {
        if path.starts_with("::") {
            path.drain(.."::".len());
        }
        self.imports.push(self.raw.directives.len());
        self.raw.directives.push(RawDirective {
            form: USE_FORM,
            spec: path,
            names: vec![name],
            line,
        });
    }

    // constraint: a path call holds a place for the resolver only where its path may start at another crate or at a
    // name a `use` binds, a name that is neither a type, written capitalised, nor a path keyword nor the standard
    // library's crates
    pub(super) fn record_written_path(&mut self, index: usize, written: &str) {
        let written = written.strip_prefix("::").unwrap_or(written);
        if names_another_crate(head(written)) {
            self.written_paths.push((index, self.raw.directives.len()));
            self.raw.directives.push(RawDirective {
                form: USE_FORM,
                spec: written.to_owned(),
                names: Vec::new(),
                line: self.raw.calls[index].line,
            });
        }
    }

    // constraint: a name the file binds through another crate keeps the place of the first directive that binds it,
    // with the path it reaches; a name the file declares at its top level, or binds to two paths, is the file's own or
    // cannot be told apart, so every directive of it is dropped
    pub(super) fn record_imports(&mut self) {
        if self.imports.is_empty() && self.written_paths.is_empty() {
            return;
        }
        let mut kept = self.kept_paths();
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

    // constraint: a path call's receiver, the last segment of its path, is bound to the path where the receiver did not
    // settle on a declaration of the file, the path's first segment read through the file's `use` of that name
    fn kept_paths(&self) -> HashMap<usize, Option<String>> {
        let directives = &self.raw.directives;
        let declared = self.declared_names();
        let mut imported: HashMap<&str, Bound<&str>> = HashMap::new();
        for &at in &self.imports {
            let directive = &directives[at];
            bind(
                &mut imported,
                &directive.names[0],
                directive.spec.as_str(),
                at,
            );
        }
        let mut bound: HashMap<&str, Bound<Cow<'_, str>>> = (imported.iter())
            .map(|(&name, held)| (name, held.map(|(path, at)| (Cow::Borrowed(path), at))))
            .collect();
        for &(call, at) in &self.written_paths {
            let written = directives[at].spec.as_str();
            let receiver = last(written);
            if self.raw.calls[call].receiver.as_deref() != Some(receiver) {
                continue;
            }
            let (first, rest) = written.split_once("::").unwrap_or((written, ""));
            let path = match imported.get(first) {
                Some(Some((path, _))) if rest.is_empty() => Cow::Borrowed(*path),
                Some(Some((path, _))) => Cow::Owned(format!("{path}::{rest}")),
                Some(None) => continue,
                None if declared.contains(first) => continue,
                None => Cow::Borrowed(written),
            };
            bind(&mut bound, receiver, path, at);
        }
        bound
            .into_iter()
            .filter_map(|(name, held)| {
                let (path, at) = held?;
                (!declared.contains(name) && names_another_crate(head(&path))).then(|| {
                    let changed = (path != directives[at].spec.as_str()).then(|| path.into_owned());
                    (at, changed)
                })
            })
            .collect()
    }

    fn declared_names(&self) -> HashSet<&str> {
        let local: HashSet<u32> = self.raw.local_symbols.iter().map(|id| id.0).collect();
        (0..)
            .zip(&self.raw.symbols)
            .filter(|(id, symbol)| {
                symbol.owner.is_none()
                    && symbol.qualified_name == symbol.name
                    && !local.contains(id)
            })
            .map(|(_, symbol)| symbol.name.as_str())
            .collect()
    }
}

// constraint: the path a name is bound to and the first directive that binds it, or `None` for a name bound to two
// paths
type Bound<P> = Option<(P, usize)>;

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

fn head(path: &str) -> &str {
    path.split_once("::").map_or(path, |(first, _)| first)
}

fn last(path: &str) -> &str {
    path.rsplit_once("::").map_or(path, |(_, last)| last)
}

fn names_another_crate(head: &str) -> bool {
    head.starts_with(|first: char| first.is_ascii_lowercase() || first == '_')
        && !matches!(
            head,
            "crate" | "self" | "super" | "std" | "core" | "alloc" | "_"
        )
}
