use std::collections::{HashMap, HashSet};
use std::iter;

use kartta_lang::path::{base, dir, join, shared_segments};
use kartta_lang::{
    Extraction, ImportBinding, IncludeRef, RawDirective, RawExtraction, Repository, Resolver,
    SymbolFlags,
};

use crate::manifest;
use crate::walk::{MOD_TEST_FORM, USE_FORM};

pub(crate) struct Crates {
    packages: HashSet<String>,
    libraries: HashMap<String, Vec<Library>>,
}

// constraint: a library is found by the name a path into it starts with, and every package that declares a library of
// that name holds one, as a workspace member and a vendored copy of it do
struct Library {
    package: String,
    root: String,
}

impl Crates {
    // constraint: a package is a directory that holds a `Cargo.toml`, whose manifest is read only for the name and the
    // root of its library, so a target that it places off the conventional layout is not a crate root here
    pub(crate) fn new(repository: &dyn Repository) -> Self {
        let mut packages = HashSet::new();
        let mut libraries: HashMap<String, Vec<Library>> = HashMap::new();
        for mut manifest in repository.files_named("Cargo.toml") {
            let library = repository
                .read(&manifest)
                .ok()
                .and_then(|content| manifest::library(&String::from_utf8_lossy(&content)));
            manifest.truncate(manifest.rfind('/').unwrap_or(0));
            let package = manifest;
            if let Some((name, path)) = library
                && let Some(root) = join(&package, path.as_deref().unwrap_or("src/lib.rs"))
            {
                libraries.entry(name).or_default().push(Library {
                    package: package.clone(),
                    root,
                });
            }
            packages.insert(package);
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

    // constraint: a path into another crate starts at its library, and each segment after the crate names the module
    // file Cargo's layout gives it until one names none: a path that ends at a module binds its file, and one that ends
    // a segment further, at an item named in lower case as a function is, binds the file of the module that holds it;
    // a longer rest, or a capitalised item, which is a type whose member a path call names, binds nothing
    fn crate_binding(
        &self,
        directive: &RawDirective,
        package: &str,
        repository: &dyn Repository,
    ) -> Option<ImportBinding> {
        let name = directive.names.first()?;
        let mut segments = directive.spec.split("::");
        let krate = segments.next()?;
        let library = self.library(krate, package)?;
        if !repository.contains(&library.root) {
            return None;
        }
        let (file, last) = Self::deepest_module(&library.root, krate, segments, repository)?;
        Some(ImportBinding {
            alias: (name != last).then(|| name.clone()),
            target_dir: Some(file),
            last_segment: last.to_owned(),
        })
    }

    fn deepest_module<'s>(
        root: &str,
        krate: &'s str,
        mut segments: impl Iterator<Item = &'s str>,
        repository: &dyn Repository,
    ) -> Option<(String, &'s str)> {
        let mut file = root.to_owned();
        let mut module_dir = String::new();
        push_module_dir(&mut module_dir, root, true);
        let mut last = krate;
        while let Some(segment) = segments.next() {
            let Some(submodule) = Self::submodule(&module_dir, segment, repository) else {
                let item = segments.next().is_none()
                    && !segment.starts_with(|first: char| first.is_ascii_uppercase());
                return item.then_some((file, segment));
            };
            file = submodule;
            module_dir.push_str(segment);
            module_dir.push('/');
            last = segment;
        }
        Some((file, last))
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
        for package in ancestors(path).filter(|package| self.packages.contains(*package)) {
            let relative = if package.is_empty() {
                path
            } else {
                &path[package.len() + 1..]
            };
            if nearest {
                let crate_root = is_crate_root(relative);
                role.module_root |= crate_root;
                role.entry = crate_root && relative != "src/lib.rs";
                role.package = package;
                nearest = false;
            }
            role.test |= is_integration_test(relative);
        }
        role
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

        let role = self.role(path);
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
                    .extend(self.crate_binding(directive, role.package, repository));
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
}

// constraint: a file is an entry when it is a crate root of its nearest package other than the library, where a
// top-level `fn main` is what the target runs; a file no package holds takes the empty path as its package
struct Role<'p> {
    test: bool,
    entry: bool,
    module_root: bool,
    package: &'p str,
}

// constraint: the crate roots of a package by Cargo's conventional layout: the library, the main binary, the build
// script, and each binary, example, integration test and benchmark as a single file or as a directory with a
// `main.rs`
fn is_crate_root(relative: &str) -> bool {
    if matches!(relative, "src/lib.rs" | "src/main.rs" | "build.rs") {
        return true;
    }
    ["src/bin/", "examples/", "tests/", "benches/"]
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
