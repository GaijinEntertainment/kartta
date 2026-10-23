use std::any::Any;
use std::cell::OnceCell;
use std::collections::HashMap;
use std::iter;

use kartta_lang::path::{base, dir, join, shared_segments};
use kartta_lang::{
    Extraction, ImportBinding, IncludeRef, RawDirective, RawExtraction, Repository, Resolver,
    SymbolFlags,
};

use crate::manifest;
use crate::walk::{MOD_TEST_FORM, USE_FORM};

// constraint: each package maps to the root of its library, where its manifest names one
#[derive(PartialEq)]
pub(crate) struct Crates {
    packages: HashMap<String, Option<String>>,
    libraries: HashMap<String, Vec<Library>>,
}

// constraint: a library is found by the name a path into it starts with, and every package that declares a library of
// that name holds one, as a workspace member and a vendored copy of it do
#[derive(PartialEq)]
struct Library {
    package: String,
    root: String,
}

impl Crates {
    // constraint: a package is a directory that holds a `Cargo.toml`, whose manifest is read only for the name and the
    // root of its library, so a target other than the library that it places off the conventional layout is not a
    // crate root here
    pub(crate) fn new(repository: &dyn Repository) -> Self {
        let mut packages = HashMap::new();
        let mut libraries: HashMap<String, Vec<Library>> = HashMap::new();
        for mut manifest in repository.files_named("Cargo.toml") {
            let library = repository
                .read(&manifest)
                .ok()
                .and_then(|content| manifest::library(&String::from_utf8_lossy(&content)));
            manifest.truncate(manifest.rfind('/').unwrap_or(0));
            let package = manifest;
            let mut library_root = None;
            if let Some((name, path)) = library
                && let Some(root) = join(&package, path.as_deref().unwrap_or("src/lib.rs"))
            {
                library_root = Some(root.clone());
                libraries.entry(name).or_default().push(Library {
                    package: package.clone(),
                    root,
                });
            }
            packages.insert(package, library_root);
        }

        Self {
            packages,
            libraries,
        }
    }

    // constraint: of the packages that declare a library of the name, the caller's own is meant, else the one that
    // shares the most leading directories with the caller's package, as a workspace member is beside a vendored copy;
    // two as near are a guess, as the versions of one crate side by side in a cargo registry are
    fn library(&self, name: &str, from: &str) -> Option<&Library> {
        let nearness = |library: &&Library| {
            if library.package == from {
                usize::MAX
            } else {
                shared_segments(&library.package, from)
            }
        };
        let named = self.libraries.get(name)?;
        let nearest = named.iter().map(|library| nearness(&library)).max()?;
        let mut nearest_ones = named.iter().filter(|library| nearness(library) == nearest);
        let library = nearest_ones.next()?;
        nearest_ones.next().is_none().then_some(library)
    }

    // constraint: a path into another crate starts at its library, one through `crate::` at the caller's crate root,
    // one through `self::` at the caller's module and one through `super::` at the module above, and each segment
    // after names the module file Cargo's layout gives it until one names none: a path that ends at a module binds its
    // file, and one that ends a segment further, at an item named in lower case as a function is, binds the file of
    // the module that holds it; a longer rest, or a capitalised item, which is a type whose member a path call names,
    // binds nothing
    fn crate_binding(
        &self,
        directive: &RawDirective,
        caller: &Caller<'_>,
        repository: &dyn Repository,
    ) -> Option<ImportBinding> {
        let name = directive.names.first()?;
        let mut segments = directive.spec.split("::").peekable();
        let head = segments.next()?;
        let start = match head {
            "crate" => caller.crate_root(self, repository)?.clone(),
            "self" => caller.own_module(),
            "super" => {
                let mut module = caller.parent(&caller.own_module(), self, repository)?;
                while segments.next_if_eq(&"super").is_some() {
                    module = caller.parent(&module, self, repository)?;
                }
                module
            }
            krate => {
                let library = self.library(krate, caller.role.package)?;
                if !repository.contains(&library.root) {
                    return None;
                }
                Module::of(&library.root, true)
            }
        };
        let (file, last) = match Self::reach(start, head, segments, repository)? {
            Reached::Module { file, name } => (file, name),
            Reached::Item { holder, name }
                if !name.starts_with(|first: char| first.is_ascii_uppercase()) =>
            {
                (holder, name)
            }
            Reached::Item { .. } => return None,
        };
        Some(ImportBinding {
            alias: (name != last).then(|| name.clone()),
            target_dir: Some(file),
            last_segment: last.to_owned(),
        })
    }

    fn reach<'s>(
        mut module: Module,
        mut last: &'s str,
        mut segments: impl Iterator<Item = &'s str>,
        repository: &dyn Repository,
    ) -> Option<Reached<'s>> {
        while let Some(segment) = segments.next() {
            let Some(submodule) = Self::submodule(&module.dir, segment, repository) else {
                return segments.next().is_none().then_some(Reached::Item {
                    holder: module.file,
                    name: segment,
                });
            };
            module.file = submodule;
            module.dir.push_str(segment);
            module.dir.push('/');
            last = segment;
        }
        Some(Reached::Module {
            file: module.file,
            name: last,
        })
    }

    fn submodule(module_dir: &str, name: &str, repository: &dyn Repository) -> Option<String> {
        let file = format!("{module_dir}{name}.rs");
        if repository.contains(&file) {
            return Some(file);
        }
        let file = format!("{module_dir}{name}/mod.rs");
        repository.contains(&file).then_some(file)
    }

    fn role<'p>(&self, path: &'p str) -> Role<'p> {
        let mut role = Role {
            test: is_integration_test(path),
            entry: false,
            module_root: matches!(base(path), "lib.rs" | "main.rs" | "mod.rs"),
            package: "",
        };
        let mut nearest = true;
        for (package, library) in
            ancestors(path).filter_map(|package| Some((package, self.packages.get(package)?)))
        {
            let relative = relative_to(package, path);
            if nearest {
                let crate_root = is_crate_root(relative);
                role.module_root |= crate_root || library.as_deref() == Some(path);
                role.entry = crate_root && relative != "src/lib.rs";
                role.package = package;
                nearest = false;
            }
            role.test |= is_integration_test(relative);
        }
        role
    }

    // constraint: `crate::` starts at the root of the caller's crate by Cargo's layout: the file itself when it is a
    // crate root, the `main.rs` of the target directory it lies in, else the nearest of the library whose directory
    // holds it and, below `src/`, the main binary; a file of `tests/`, `examples/` or `benches/` outside a target
    // directory is shared by the targets beside it, so no one file is its crate root
    fn crate_root(&self, path: &str, package: &str, repository: &dyn Repository) -> Option<Module> {
        let relative = relative_to(package, path);
        if is_crate_root(relative) {
            return Some(Module::of(path, true));
        }
        let root = if let Some((targets, rest)) = TARGET_DIRS
            .iter()
            .find_map(|targets| Some((targets, relative.strip_prefix(targets)?)))
        {
            let (target, _) = rest.split_once('/')?;
            join(package, &format!("{targets}{target}/main.rs"))?
        } else if let Some(Some(library)) = self.packages.get(package)
            && let library_dir = library.rsplit_once('/').map_or("", |(dir, _)| dir)
            && is_below(library_dir, path)
            && (library_dir != package || !relative.starts_with("src/"))
            && repository.contains(library)
        {
            library.clone()
        } else if relative.starts_with("src/") {
            join(package, "src/main.rs")?
        } else {
            return None;
        };
        repository.contains(&root).then(|| Module::of(&root, true))
    }

    // constraint: the spec is the module's path from the declaring file's module directory, one directory per inline
    // module around the declaration; a `#[path]` override is relative to the declaring file's directory, or inside an
    // inline module to that module's directory
    fn module_file(
        directive: &RawDirective,
        path: &str,
        module_root: bool,
        repository: &dyn Repository,
    ) -> Option<String> {
        if let Some(spec) = directive.names.first() {
            let mut from = String::new();
            match directive.spec.rsplit_once('/') {
                Some((inline, _)) => {
                    push_module_dir(&mut from, path, module_root);
                    from.push_str(inline);
                }
                None => from.push_str(dir(path)),
            }
            return join(&from, spec).filter(|file| repository.contains(file));
        }

        let mut file = String::with_capacity(path.len() + directive.spec.len() + "/mod.rs".len());
        push_module_dir(&mut file, path, module_root);
        file.push_str(&directive.spec);
        file.push_str(".rs");
        if repository.contains(&file) {
            return Some(file);
        }
        file.truncate(file.len() - ".rs".len());
        file.push_str("/mod.rs");
        repository.contains(&file).then_some(file)
    }
}

// constraint: the file whose paths resolve, and the root of its crate, looked up once a path through `crate::` or
// `super::` asks for it
struct Caller<'p> {
    path: &'p str,
    role: Role<'p>,
    root: OnceCell<Option<Module>>,
}

impl Caller<'_> {
    fn own_module(&self) -> Module {
        Module::of(self.path, self.role.module_root)
    }

    fn crate_root(&self, crates: &Crates, repository: &dyn Repository) -> Option<&Module> {
        self.root
            .get_or_init(|| crates.crate_root(self.path, self.role.package, repository))
            .as_ref()
    }

    // constraint: a module's parent holds the directory its own directory lies in: the crate root where that is the
    // root's directory, else the file Cargo's layout names for it; the crate root has none
    fn parent(
        &self,
        module: &Module,
        crates: &Crates,
        repository: &dyn Repository,
    ) -> Option<Module> {
        let root = self.crate_root(crates, repository);
        if root.is_some_and(|root| root.dir == module.dir) {
            return None;
        }
        let own = module.dir.strip_suffix('/')?;
        let above = own.rsplit_once('/').map_or("", |(above, _)| above);
        let dir = if above.is_empty() {
            String::new()
        } else {
            format!("{above}/")
        };
        if let Some(root) = root.filter(|root| root.dir == dir) {
            return Some(root.clone());
        }
        if above.is_empty() {
            return None;
        }
        let file = format!("{above}.rs");
        if repository.contains(&file) {
            return Some(Module { file, dir });
        }
        let file = format!("{dir}mod.rs");
        repository.contains(&file).then_some(Module { file, dir })
    }
}

// constraint: a path's segments name modules until one names none, so it ends at the file of the module its last
// segment names, or at an item of the module before it, held by that module's file
enum Reached<'s> {
    Module { file: String, name: &'s str },
    Item { holder: String, name: &'s str },
}

// constraint: a module is the file that holds it and the directory that holds its submodules' files
#[derive(Clone)]
struct Module {
    file: String,
    dir: String,
}

impl Module {
    fn of(file: &str, module_root: bool) -> Self {
        let mut dir = String::new();
        push_module_dir(&mut dir, file, module_root);
        Self {
            file: file.to_owned(),
            dir,
        }
    }
}

// constraint: a file named `lib.rs`, `main.rs` or `mod.rs`, or a crate root, keeps its submodules beside it, and any
// other file in the directory named for its stem
fn push_module_dir(into: &mut String, path: &str, module_root: bool) {
    let (parent, file) = path.rsplit_once('/').unwrap_or(("", path));
    if !parent.is_empty() {
        into.push_str(parent);
        into.push('/');
    }
    if !module_root {
        into.push_str(file.rsplit_once('.').map_or(file, |(stem, _)| stem));
        into.push('/');
    }
}

impl Resolver for Crates {
    fn resolve(
        &self,
        mut raw: RawExtraction,
        path: &str,
        repository: &dyn Repository,
    ) -> Extraction {
        let entry_candidates = std::mem::take(&mut raw.entry_candidates);
        let (mut extraction, directives) = raw.into_extraction(path);

        let caller = Caller {
            path,
            role: self.role(path),
            root: OnceCell::new(),
        };
        let role = &caller.role;
        let flags = SymbolFlags::empty()
            .with_test(role.test)
            .with_generated(is_generated_path(path));
        extraction.flag_symbols(flags);
        for id in entry_candidates.into_iter().filter(|_| role.entry) {
            if let Some(symbol) = extraction.symbols.get_mut(id.0 as usize) {
                symbol.flags = symbol.flags.with_entry(true);
            }
        }

        for directive in &directives {
            if directive.form == USE_FORM {
                extraction
                    .bindings
                    .extend(self.crate_binding(directive, &caller, repository));
                continue;
            }
            let Some(target) = Self::module_file(directive, path, role.module_root, repository)
            else {
                continue;
            };
            if directive.form == MOD_TEST_FORM || role.test {
                extraction.test_includes.push(target.clone());
            }
            extraction.includes.push(IncludeRef { target });
        }

        extraction
    }

    fn resolves_as(&self, kept: &dyn Resolver) -> bool {
        (kept as &dyn Any).downcast_ref::<Self>() == Some(self)
    }
}

// constraint: a file is an entry when it is a crate root of its nearest package other than the library, where a
// top-level `fn main` is what the target runs; a file no package holds takes the empty path as its package
struct Role<'p> {
    test: bool,
    entry: bool,
    module_root: bool,
    package: &'p str,
}

const TARGET_DIRS: [&str; 4] = ["src/bin/", "examples/", "tests/", "benches/"];

// constraint: the crate roots of a package by Cargo's conventional layout: the library, the main binary, the build
// script, and each binary, example, integration test and benchmark as a single file or as a directory with a
// `main.rs`
fn is_crate_root(relative: &str) -> bool {
    if matches!(relative, "src/lib.rs" | "src/main.rs" | "build.rs") {
        return true;
    }
    TARGET_DIRS
        .iter()
        .find_map(|targets| relative.strip_prefix(targets))
        .is_some_and(|target| {
            target
                .split_once('/')
                .is_none_or(|(_, file)| file == "main.rs")
        })
}

// constraint: below the repository root or a package around the file, a `tests` directory outside `src` holds
// integration tests and their support; this covers a crate whose manifest the tree lacks, and a fixture crate kept
// under `tests/`, while a `tests` module inside `src` stays production code
fn is_integration_test(relative: &str) -> bool {
    relative
        .split('/')
        .take_while(|component| *component != "src")
        .any(|component| component == "tests")
}

fn relative_to<'p>(package: &str, path: &'p str) -> &'p str {
    if package.is_empty() {
        path
    } else {
        &path[package.len() + 1..]
    }
}

fn is_below(dir: &str, path: &str) -> bool {
    dir.is_empty()
        || path
            .strip_prefix(dir)
            .is_some_and(|rest| rest.starts_with('/'))
}

fn ancestors(path: &str) -> impl Iterator<Item = &str> {
    iter::successors(Some(path), |at| {
        (!at.is_empty()).then(|| at.rsplit_once('/').map_or("", |(parent, _)| parent))
    })
    .skip(1)
}

fn is_generated_path(path: &str) -> bool {
    let name = base(path);
    name.ends_with(".gen.rs") || name.ends_with("_generated.rs")
}
